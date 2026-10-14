import { useCallback, useEffect, useRef, useState } from 'react'
import { throwIfAborted } from '@/lib/abort-signal'
import {
  cancelLocalSpeechModelDownload,
  downloadLocalSpeechModel,
  getLocalSpeechModels,
  requiredSpeechModels,
  type SpeechModelCatalog,
  type SpeechModelKind,
} from './speech-models'

type Pending = {
  signal: AbortSignal
  abort: () => void
  resolve: () => void
  reject: (error: unknown) => void
}
const cancelled = () => new DOMException('Speech setup cancelled.', 'AbortError')

export function useSpeechModels(kinds: readonly SpeechModelKind[]) {
  const [catalog, setCatalog] = useState<SpeechModelCatalog | null>(null)
  const [open, setOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const pending = useRef<Pending | null>(null)
  const mounted = useRef(false)
  const epoch = useRef(0)
  const refreshSequence = useRef(0)
  const actionSequence = useRef(0)
  const latestKinds = useRef(kinds)
  latestKinds.current = kinds

  const settle = useCallback((error?: unknown) => {
    epoch.current++
    refreshSequence.current++
    const request = pending.current
    pending.current = null
    if (request) {
      request.signal.removeEventListener('abort', request.abort)
      if (error) request.reject(error)
      else request.resolve()
    }
    if (mounted.current) {
      setOpen(false)
      setLoading(false)
    }
  }, [])

  const apply = useCallback(
    (next: SpeechModelCatalog) => {
      if (!mounted.current) return
      setCatalog(next)
      setError('')
      if (
        pending.current &&
        requiredSpeechModels(next, latestKinds.current).every(
          (model) => model.status === 'installed',
        )
      )
        settle()
    },
    [settle],
  )

  const refresh = useCallback(
    async (signal?: AbortSignal) => {
      const generation = epoch.current
      const sequence = ++refreshSequence.current
      const current = () =>
        mounted.current &&
        generation === epoch.current &&
        sequence === refreshSequence.current &&
        !signal?.aborted
      try {
        const next = await getLocalSpeechModels(signal)
        if (current()) apply(next)
      } catch (caught) {
        if (current()) throw caught
      }
    },
    [apply],
  )

  useEffect(() => {
    mounted.current = true
    void refresh().catch(() => {})
    return () => {
      mounted.current = false
      settle(cancelled())
    }
  }, [settle, refresh])

  useEffect(() => {
    if (!open) return
    const controller = new AbortController()
    let timer: ReturnType<typeof setTimeout>
    const poll = async () => {
      try {
        await refresh(controller.signal)
      } catch (caught) {
        if (!controller.signal.aborted)
          setError(caught instanceof Error ? caught.message : String(caught))
      } finally {
        if (!controller.signal.aborted) timer = setTimeout(() => void poll(), 750)
      }
    }
    void poll()
    return () => {
      controller.abort()
      clearTimeout(timer)
    }
  }, [open, refresh])

  const ensureReady = useCallback(
    async (signal: AbortSignal) => {
      throwIfAborted(signal)
      if (!mounted.current) throw cancelled()
      settle(cancelled())
      const sequence = ++refreshSequence.current
      return new Promise<void>((resolve, reject) => {
        const abort = () => {
          if (pending.current === request) settle(signal.reason || cancelled())
        }
        // 目录请求之前即登记所有权，关闭或后来的请求能立即作废这次初始化。
        const request = { signal, abort, resolve, reject }
        pending.current = request
        signal.addEventListener('abort', abort, { once: true })
        if (signal.aborted) {
          abort()
          return
        }
        void getLocalSpeechModels(signal).then(
          (next) => {
            if (!mounted.current || pending.current !== request || signal.aborted) return
            if (sequence === refreshSequence.current) apply(next)
            if (pending.current === request) setOpen(true)
          },
          (caught) => {
            if (pending.current === request) settle(caught)
          },
        )
      })
    },
    [apply, settle],
  )

  const run = useCallback(
    async (action: () => Promise<unknown>) => {
      const generation = epoch.current
      const sequence = ++actionSequence.current
      const current = () =>
        mounted.current && generation === epoch.current && sequence === actionSequence.current
      setLoading(true)
      setError('')
      try {
        await action()
        if (current()) await refresh()
      } catch (caught) {
        if (current()) setError(caught instanceof Error ? caught.message : String(caught))
      } finally {
        if (current()) setLoading(false)
      }
    },
    [refresh],
  )

  const selectedVoice = catalog?.defaults.voice || ''

  return {
    catalog,
    open,
    loading,
    error,
    selectedVoice,
    ensureReady,
    models: catalog ? requiredSpeechModels(catalog, kinds) : [],
    show: () => {
      settle(cancelled())
      setOpen(true)
      setError('')
    },
    close: () => settle(cancelled()),
    download: (id: string) => run(() => downloadLocalSpeechModel(id)),
    cancelDownload: (id: string) => run(() => cancelLocalSpeechModelDownload(id)),
    downloadAll: () =>
      run(async () => {
        if (!catalog) return
        for (const model of requiredSpeechModels(catalog, latestKinds.current)) {
          if (!['installed', 'downloading', 'verifying'].includes(model.status))
            await downloadLocalSpeechModel(model.id)
        }
      }),
  }
}
