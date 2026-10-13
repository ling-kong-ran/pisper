import { requestJson } from '@/lib/http/http'
import { translateText } from '@/app/i18n/i18n'
import type {
  DesktopTerminalCreated,
  DesktopTerminalCreateOptions,
  DesktopTerminalEvent,
  DesktopTerminalProfile,
} from '@/types/update'

// 终端领域的两种传输共用面板；网页连接只拥有自己的 PTY，关闭面板不会影响其他页面。
type CreateInput = DesktopTerminalCreateOptions & { sessionId: string }
type Transport = {
  terminalProfiles: () => Promise<DesktopTerminalProfile[]>
  terminalCreate: (
    input: CreateInput,
    onEvent: (event: DesktopTerminalEvent) => void,
  ) => Promise<DesktopTerminalCreated>
  terminalWrite: (id: string, data: Uint8Array) => Promise<void>
  terminalResize: (id: string, cols: number, rows: number) => Promise<void>
  terminalClose: (id: string) => Promise<boolean>
  terminalCloseAll: () => Promise<number>
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw protocolError()
  return value as Record<string, unknown>
}

function protocolError() {
  return new Error(translateText('terminal:terminalPanel.invalidResponse'))
}

function parseProfiles(value: unknown): DesktopTerminalProfile[] {
  if (!Array.isArray(value)) throw protocolError()
  return value.map((item: unknown) => {
    const profile = record(item)
    if (
      typeof profile.id !== 'string' ||
      typeof profile.label !== 'string' ||
      typeof profile.default !== 'boolean'
    )
      throw protocolError()
    return { id: profile.id, label: profile.label, default: profile.default }
  })
}

export function createTerminalTransport(): Transport {
  const native = window.pisperDesktop
  if (
    native?.terminalProfiles &&
    native.terminalCreate &&
    native.terminalWrite &&
    native.terminalResize &&
    native.terminalClose &&
    native.terminalCloseAll
  ) {
    return {
      terminalProfiles: native.terminalProfiles,
      terminalCreate: native.terminalCreate,
      terminalWrite: native.terminalWrite,
      terminalResize: native.terminalResize,
      terminalClose: native.terminalClose,
      terminalCloseAll: native.terminalCloseAll,
    }
  }
  const sockets = new Map<string, WebSocket>()
  const send = async (id: string, value: unknown) => {
    const socket = sockets.get(id)
    if (socket?.readyState !== WebSocket.OPEN)
      throw new Error(translateText('terminal:terminalPanel.disconnected'))
    socket.send(JSON.stringify(value))
  }
  const close = async (id: string) => {
    const socket = sockets.get(id)
    if (!socket) return false
    sockets.delete(id)
    socket.close()
    return true
  }
  return {
    terminalProfiles: () => requestJson('/api/terminal/profiles', { parse: parseProfiles }),
    terminalCreate: (input, onEvent) =>
      new Promise((resolve, reject) => {
        const url = new URL('/api/terminal/connect', location.href)
        url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
        const socket = new WebSocket(url)
        sockets.set(input.terminalId, socket)
        let created = false
        let finished = false
        const fail = (error: Error) => {
          if (finished) return
          finished = true
          if (!created) reject(error)
          else onEvent({ type: 'error', terminalId: input.terminalId, message: error.message })
          socket.close()
        }
        const timeout = setTimeout(
          () => fail(new Error(translateText('terminal:terminalPanel.connectionTimeout'))),
          15_000,
        )
        socket.onopen = () =>
          socket.send(
            JSON.stringify({
              type: 'create',
              terminalId: input.terminalId,
              profileId: input.profileId,
              sessionId: input.sessionId,
              cols: input.cols,
              rows: input.rows,
            }),
          )
        socket.onmessage = ({ data }: MessageEvent<unknown>) => {
          try {
            if (typeof data !== 'string') throw protocolError()
            const event = record(JSON.parse(data))
            if (event.type === 'created' && !created) {
              const terminal = record(event.terminal)
              if (
                terminal.terminalId !== input.terminalId ||
                typeof terminal.profileId !== 'string' ||
                typeof terminal.cwd !== 'string'
              )
                throw protocolError()
              created = true
              clearTimeout(timeout)
              resolve({
                terminalId: input.terminalId,
                profileId: terminal.profileId,
                cwd: terminal.cwd,
              })
            } else if (event.type === 'error' && typeof event.message === 'string') {
              fail(new Error(event.message))
            } else if (
              created &&
              event.terminalId === input.terminalId &&
              event.type === 'output' &&
              Array.isArray(event.data) &&
              event.data.every(
                (byte: unknown) =>
                  typeof byte === 'number' && Number.isInteger(byte) && byte >= 0 && byte <= 255,
              )
            ) {
              onEvent({ type: 'output', terminalId: input.terminalId, data: event.data })
            } else if (
              created &&
              event.terminalId === input.terminalId &&
              event.type === 'exit' &&
              (event.code === null ||
                (typeof event.code === 'number' && Number.isInteger(event.code)))
            ) {
              finished = true
              onEvent({ type: 'exit', terminalId: input.terminalId, code: event.code })
            } else throw protocolError()
          } catch (error) {
            fail(error instanceof Error ? error : protocolError())
          }
        }
        socket.onerror = () => fail(new Error(translateText('terminal:terminalPanel.disconnected')))
        socket.onclose = () => {
          clearTimeout(timeout)
          if (sockets.get(input.terminalId) === socket) {
            sockets.delete(input.terminalId)
            if (!finished) fail(new Error(translateText('terminal:terminalPanel.disconnected')))
          } else if (!created)
            reject(new Error(translateText('terminal:terminalPanel.disconnected')))
        }
      }),
    terminalWrite: (id, data) => send(id, { type: 'input', data: Array.from(data) }),
    terminalResize: (id, cols, rows) => send(id, { type: 'resize', cols, rows }),
    terminalClose: close,
    terminalCloseAll: async () => {
      const ids = [...sockets.keys()]
      await Promise.all(ids.map(close))
      return ids.length
    },
  }
}
