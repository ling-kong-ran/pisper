import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { chatErrorMessage } from './chat-errors'
import { clearComposerDraft } from './composer-drafts'
import { ensureSideChat, getSideChat, type SideChatResponse } from './side-chat-api'
import {
  SideChatContext,
  EMPTY_ENTRY,
  type SideChatEntry,
  type SideChatRuntime,
} from './side-chat-context'

// 只拥有临时会话与主会话的关联、草稿和请求状态。正文仍由聊天会话状态管理器唯一持有。
// 面板关闭不会停止后台会话；只有这个路由所有者卸载时取消元数据请求。
export function SideChatProvider({
  runtime,
  children,
}: {
  runtime: SideChatRuntime
  children: ReactNode
}) {
  const [entries, setEntries] = useState<Record<string, SideChatEntry>>({})
  const entriesRef = useRef(entries)
  const mounted = useRef(true)
  const runtimeRef = useRef(runtime)
  const leases = useRef(new Map<string, () => void>())
  const requests = useRef(
    new Map<
      string,
      { create: boolean; controller: AbortController; promise: Promise<SideChatResponse | null> }
    >(),
  )
  useLayoutEffect(() => {
    runtimeRef.current = runtime
  }, [runtime])
  // StrictMode 会重放被动 effect；在子面板重新请求前恢复路由所有权。
  useLayoutEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const update = useCallback((parentId: string, patch: Partial<SideChatEntry>) => {
    const next = {
      ...entriesRef.current,
      [parentId]: { ...(entriesRef.current[parentId] ?? EMPTY_ENTRY), ...patch },
    }
    entriesRef.current = next
    setEntries(next)
  }, [])
  const load = useCallback(
    async (parentId: string, create = false): Promise<SideChatResponse | null> => {
      if (!mounted.current) return null
      const pending = requests.current.get(parentId)
      if (pending) {
        const response = await pending.promise
        if (!create || pending.create) return response
        return load(parentId, true)
      }
      const controller = new AbortController()
      update(parentId, { loading: true, error: '' })
      const promise = (async () => {
        try {
          const response = await (create ? ensureSideChat : getSideChat)(
            parentId,
            controller.signal,
          )
          if (controller.signal.aborted) return null
          const previousId = entriesRef.current[parentId]?.session?.id
          if (previousId && previousId !== response.session?.id) {
            runtimeRef.current.discard(previousId)
            leases.current.get(previousId)?.()
            leases.current.delete(previousId)
            clearComposerDraft(previousId)
          }
          update(parentId, {
            ...response,
            expired: response.session
              ? false
              : Boolean(previousId || entriesRef.current[parentId]?.expired),
          })
          if (response.session) {
            if (!leases.current.has(response.session.id))
              leases.current.set(
                response.session.id,
                runtimeRef.current.retainSessionState(response.session.id),
              )
            // 重新打开时先恢复审批/运行状态；普通同步会避让仍由本地持有的 SSE。
            await runtimeRef.current.syncLiveSession(response.session.id)
            if (controller.signal.aborted) return null
            await runtimeRef.current.loadSessionMessages(response.session.id)
          }
          return controller.signal.aborted ? null : response
        } catch (error) {
          if (!controller.signal.aborted) update(parentId, { error: chatErrorMessage(error) })
          return null
        } finally {
          if (!controller.signal.aborted) update(parentId, { loading: false })
          if (requests.current.get(parentId)?.controller === controller)
            requests.current.delete(parentId)
        }
      })()
      requests.current.set(parentId, { create, controller, promise })
      return promise
    },
    [update],
  )
  useEffect(() => {
    mounted.current = true
    const pendingRequests = requests.current
    const retainedStates = leases.current
    const refreshExpired = () => {
      // 运行状态以服务端为准：断线恢复后的本地 streaming 可能已过时。
      // GET 会保护仍在执行/审批中的会话，不续期，也不会中止它。
      for (const [parentId, entry] of Object.entries(entriesRef.current)) {
        if (
          entry.expiresAt &&
          Date.parse(entry.expiresAt) <= Date.now() &&
          !requests.current.has(parentId)
        ) {
          void load(parentId)
        }
      }
    }
    const timer = window.setInterval(refreshExpired, 60_000)
    const onVisible = () => {
      if (document.visibilityState === 'visible') refreshExpired()
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      mounted.current = false
      window.clearInterval(timer)
      document.removeEventListener('visibilitychange', onVisible)
      for (const request of pendingRequests.values()) request.controller.abort()
      pendingRequests.clear()
      for (const release of retainedStates.values()) release()
      retainedStates.clear()
    }
  }, [load])
  const setDraft = useCallback(
    (parentId: string, draft: string) => update(parentId, { draft }),
    [update],
  )
  const value = useMemo(
    () => ({ entries, runtime, load, setDraft }),
    [entries, runtime, load, setDraft],
  )
  return <SideChatContext.Provider value={value}>{children}</SideChatContext.Provider>
}
