// Route-independent owner for deferred runtime preferences. No work starts on import.
// Only read-only probes retry while the session is active; each mutation runs once.
import type { ModelOption } from '@/types/chat'

export type RuntimeSelection = { model?: ModelOption; thinkingLevel?: string }
export type RuntimeSelectionEntry = {
  selection: RuntimeSelection
  status: 'pending' | 'applying' | 'success' | 'error'
  result?: Record<string, unknown>
  error?: unknown
}

type Dependencies = {
  isStreaming: (id: string, signal: AbortSignal) => Promise<boolean>
  apply: (id: string, selection: RuntimeSelection) => Promise<Record<string, unknown>>
  intervalMs?: number
}

function waitForNextProbe(ms: number, signal: AbortSignal) {
  return new Promise<void>((resolve) => {
    const done = () => {
      clearTimeout(timer)
      signal.removeEventListener('abort', done)
      resolve()
    }
    const timer = setTimeout(done, ms)
    signal.addEventListener('abort', done, { once: true })
    if (signal.aborted) done()
  })
}

export function createRuntimeSelectionQueue({
  isStreaming,
  apply,
  intervalMs = 750,
}: Dependencies) {
  const entries = new Map<string, RuntimeSelectionEntry>()
  const jobs = new Map<string, { controller: AbortController; promise: Promise<void> }>()
  const listeners = new Set<(id: string, entry?: RuntimeSelectionEntry) => void>()
  const emit = (id: string) => {
    const entry = entries.get(id)
    for (const listener of listeners) listener(id, entry)
    // Undelivered results survive a route transition; delivered results need no cache.
    if (
      listeners.size &&
      entries.get(id) === entry &&
      (entry?.status === 'success' || entry?.status === 'error')
    )
      entries.delete(id)
  }

  const cancel = (id: string) => {
    const job = jobs.get(id)
    job?.controller.abort()
    jobs.delete(id)
    entries.delete(id)
    emit(id)
  }

  const select = (id: string, selection?: RuntimeSelection) => {
    if (entries.get(id)?.status === 'applying') return false
    if (!selection) {
      cancel(id)
      return true
    }
    entries.set(id, { selection, status: 'pending' })
    emit(id)
    if (jobs.has(id)) return true
    const controller = new AbortController()
    const { signal } = controller
    const job = { controller, promise: Promise.resolve() }
    jobs.set(id, job)
    job.promise = (async () => {
      try {
        while (!signal.aborted) {
          const streaming = await isStreaming(id, signal)
          if (signal.aborted) return
          if (streaming) {
            await waitForNextProbe(intervalMs, signal)
            continue
          }
          // A newer selection can replace the original one while a probe was in flight.
          const entry = entries.get(id)
          if (!entry) return
          entries.set(id, { ...entry, status: 'applying' })
          emit(id)
          const result = await apply(id, entry.selection)
          if (!signal.aborted) {
            if (jobs.get(id) === job) jobs.delete(id)
            entries.set(id, { ...entry, status: 'success', result })
            emit(id)
          }
          return
        }
      } catch (error) {
        const entry = entries.get(id)
        if (!signal.aborted && entry) {
          if (jobs.get(id) === job) jobs.delete(id)
          entries.set(id, { ...entry, status: 'error', error })
          emit(id)
        }
      } finally {
        if (jobs.get(id) === job) jobs.delete(id)
      }
    })()
    return true
  }

  return {
    select,
    subscribe(listener: (id: string, entry?: RuntimeSelectionEntry) => void) {
      listeners.add(listener)
      for (const [id, entry] of entries) {
        listener(id, entry)
        if (entries.get(id) === entry && (entry.status === 'success' || entry.status === 'error'))
          entries.delete(id)
      }
      return () => {
        listeners.delete(listener)
      }
    },
    async dispose() {
      const pending = [...jobs.values()]
      listeners.clear()
      for (const id of jobs.keys()) cancel(id)
      entries.clear()
      await Promise.all(pending.map((job) => job.promise))
    },
  }
}
