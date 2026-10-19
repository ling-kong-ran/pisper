// PTY 的所有权属于应用生命周期；订阅者只观察资源，退订不代表停止进程。
import type {
  DesktopTerminalCreateOptions,
  DesktopTerminalCreated,
  DesktopTerminalEvent,
} from '@/types/update'

export type TerminalTransport = {
  terminalCreate: (
    options: DesktopTerminalCreateOptions,
    onEvent: (event: DesktopTerminalEvent) => void,
  ) => Promise<DesktopTerminalCreated>
  terminalWrite: (id: string, data: Uint8Array) => Promise<void>
  terminalResize: (id: string, cols: number, rows: number) => Promise<void>
  terminalClose: (id: string) => Promise<boolean>
}
export type TerminalRecord = {
  id: string
  sessionId: string
  cwd: string
  profileId: string
  orphaned: boolean
  status: 'starting' | 'running' | 'stopping' | 'exited' | 'error'
  exitCode: number | null
  error: string
  output: string
  truncated: boolean
}
export type TerminalOutputChunk = { sequence: number; data: Uint8Array }
const MAX_OUTPUT_BYTES = 2 * 1024 * 1024
const MAX_LOG_CHARACTERS = 512 * 1024

// 以流式状态机跨块处理 ANSI/OSC，避免日志短暂显示半条控制序列或正则反复回溯。
function createLogDecoder() {
  const decoder = new TextDecoder()
  let state: 'text' | 'escape' | 'csi' | 'osc' | 'osc-escape' | 'charset' = 'text'
  return (bytes: Uint8Array) => {
    let output = ''
    for (const char of decoder.decode(bytes, { stream: true })) {
      const code = char.codePointAt(0)!
      if (state === 'text') {
        if (code === 27) state = 'escape'
        else if (code === 155) state = 'csi'
        else if (code === 157) state = 'osc'
        else if (code === 9 || code === 10 || code === 13 || (code >= 32 && code !== 127))
          output += char
      } else if (state === 'escape') {
        state =
          char === '[' ? 'csi' : char === ']' ? 'osc' : '()*+'.includes(char) ? 'charset' : 'text'
      } else if (state === 'csi') {
        if (code >= 64 && code <= 126) state = 'text'
      } else if (state === 'charset') state = 'text'
      else if (state === 'osc') {
        if (code === 7) state = 'text'
        else if (code === 27) state = 'osc-escape'
      } else if (state === 'osc-escape')
        state = char === '\\' ? 'text' : code === 27 ? 'osc-escape' : 'osc'
    }
    return output
  }
}

export function createTerminalRegistry(transport: TerminalTransport) {
  const records = new Map<string, TerminalRecord>()
  const outputs = new Map<
    string,
    {
      chunks: TerminalOutputChunk[]
      bytes: number
      sequence: number
      decoder: ReturnType<typeof createLogDecoder>
      text: string
    }
  >()
  const live = new Set<string>()
  const creating = new Map<string, Promise<string>>()
  const stopping = new Map<string, Promise<void>>()
  const listeners = new Set<() => void>()
  const eventListeners = new Set<(event: DesktopTerminalEvent) => void>()
  const snapshots = new Map<string | undefined, readonly TerminalRecord[]>()
  let disposed = false
  let disposal: Promise<void> | undefined
  const snapshot = (sessionId?: string): readonly TerminalRecord[] => {
    if (!snapshots.has(sessionId))
      snapshots.set(
        sessionId,
        [...records.values()].filter(
          (record) => sessionId === undefined || record.sessionId === sessionId,
        ),
      )
    return snapshots.get(sessionId)!
  }
  const publish = () => {
    for (const [key, previous] of snapshots) {
      const next = [...records.values()].filter(
        (record) => key === undefined || record.sessionId === key,
      )
      if (
        next.length !== previous.length ||
        next.some((record, index) => record !== previous[index])
      )
        snapshots.set(key, next)
    }
    for (const listener of listeners) listener()
  }
  const update = (id: string, patch: Partial<TerminalRecord>) => {
    const current = records.get(id)
    if (!current) return
    records.set(id, { ...current, ...patch })
    publish()
  }
  const receive = (id: string, event: DesktopTerminalEvent) => {
    if (disposed || event.terminalId !== id || !live.has(id)) return
    if (event.type === 'output') {
      if (
        !Array.isArray(event.data) ||
        event.data.some((byte) => !Number.isInteger(byte) || byte < 0 || byte > 255)
      )
        return
      const output = outputs.get(id)!
      const data = Uint8Array.from(event.data.slice(-MAX_OUTPUT_BYTES))
      output.chunks.push({ sequence: ++output.sequence, data })
      output.bytes += data.length
      let truncated = records.get(id)!.truncated || event.data.length > MAX_OUTPUT_BYTES
      while (output.bytes > MAX_OUTPUT_BYTES && output.chunks.length > 1) {
        output.bytes -= output.chunks.shift()!.data.length
        truncated = true
      }
      output.text += output.decoder(data)
      if (output.text.length > MAX_LOG_CHARACTERS) {
        output.text = output.text.slice(-MAX_LOG_CHARACTERS).replace(/^[\uDC00-\uDFFF]/, '')
        truncated = true
      }
      update(id, { output: output.text, truncated })
    } else if (event.type === 'exit') {
      live.delete(id)
      update(id, { status: 'exited', exitCode: event.code, error: '' })
    } else if (event.type === 'error') update(id, { status: 'error', error: event.message })
    for (const listener of eventListeners) listener(event)
  }
  const create = async (sessionId: string, cwd: string, profileId: string) => {
    if (disposed) throw new Error('Terminal registry disposed')
    const id = `terminal-${crypto.randomUUID()}`
    records.set(id, {
      id,
      sessionId,
      cwd,
      profileId,
      orphaned: false,
      status: 'starting',
      exitCode: null,
      error: '',
      output: '',
      truncated: false,
    })
    outputs.set(id, { chunks: [], bytes: 0, sequence: 0, decoder: createLogDecoder(), text: '' })
    live.add(id)
    publish()
    const pending = (async () => {
      try {
        const created = await transport.terminalCreate(
          { terminalId: id, profileId, cwd, cols: 100, rows: 24 },
          (event) => receive(id, event),
        )
        if (created.terminalId !== id)
          throw new Error('Terminal creation returned a different identity')
        update(id, {
          cwd: created.cwd,
          profileId: created.profileId,
          ...(!disposed && records.get(id)?.status === 'starting'
            ? { status: 'running' as const }
            : {}),
        })
        return id
      } catch (error) {
        update(id, {
          status: 'error',
          error: error instanceof Error ? error.message : String(error),
        })
        throw error
      } finally {
        creating.delete(id)
      }
    })()
    creating.set(id, pending)
    return pending
  }
  const stop = (id: string): Promise<void> => {
    const existing = stopping.get(id)
    if (existing) return existing
    if (!live.has(id)) return Promise.resolve()
    update(id, { status: 'stopping', error: '' })
    const pending = (async () => {
      try {
        const creation = creating.get(id)
        if (creation) await creation.catch(() => undefined)
        if (!live.has(id)) return
        await transport.terminalClose(id)
        live.delete(id)
        update(id, { status: 'exited', exitCode: null, error: '' })
      } catch (error) {
        update(id, {
          status: 'error',
          error: error instanceof Error ? error.message : String(error),
        })
        throw error
      } finally {
        stopping.delete(id)
      }
    })()
    stopping.set(id, pending)
    return pending
  }
  return {
    create,
    stop,
    snapshot,
    subscribe(listener: () => void) {
      listeners.add(listener)
      return () => {
        listeners.delete(listener)
      }
    },
    subscribeEvents(listener: (event: DesktopTerminalEvent) => void) {
      eventListeners.add(listener)
      return () => {
        eventListeners.delete(listener)
      }
    },
    outputChunks(id: string): readonly TerminalOutputChunk[] {
      return outputs.get(id)?.chunks || []
    },
    async write(id: string, data: Uint8Array) {
      if (disposed || !live.has(id)) throw new Error('Terminal is not running')
      await transport.terminalWrite(id, data)
    },
    async resize(id: string, cols: number, rows: number) {
      if (
        disposed ||
        !live.has(id) ||
        !Number.isInteger(cols) ||
        !Number.isInteger(rows) ||
        cols < 1 ||
        rows < 1
      )
        return
      await transport.terminalResize(id, cols, rows)
    },
    orphan(sessionIds: readonly string[]) {
      const deleted = new Set(sessionIds)
      for (const record of records.values())
        if (deleted.has(record.sessionId) && !record.orphaned)
          records.set(record.id, { ...record, orphaned: true })
      publish()
    },
    dispose() {
      if (disposal) return disposal
      disposed = true
      disposal = (async () => {
        const results = await Promise.allSettled([...live].map(stop))
        listeners.clear()
        eventListeners.clear()
        const failed = results.find((result) => result.status === 'rejected')
        if (failed?.status === 'rejected') throw failed.reason
      })()
      return disposal
    },
  }
}
export type TerminalRegistry = ReturnType<typeof createTerminalRegistry>
