// 会话选中：聊天页只挂一个会话视图（会话页由路由决定显示哪个会话），这里把“打开某个会话”的请求
// 交给路由或活动会话，并在会话被删除时清掉它的上下文页与未发送草稿。
// 第 3 期起 Dockview 多标签与分屏不再挂载，A4 随手机外壳一起删除了分屏的布局持久化与面板管理
// （旧的 use-chat-dock）；Runtime 的布局存储接口保持不变，只是前端不再读写。
import { useCallback, useEffect, useRef } from 'react'
import { STORAGE_KEYS } from '@/app/storage'
import { clearComposerDraft } from './composer-drafts'
import { SESSION_SELECTED_EVENT, subscribeSessionDeletionUpdates } from './events'
import type { SessionOpenRequest } from './session-open-request'
import { useContextPagesStore } from './session-context-pages-store'
import { FOCUS_MESSAGE_PAGE_SIZE } from './use-session-catalog'

type SessionSelectionOptions = {
  setActiveId: (id: string) => void
  loadSessionMessages: (id: string, options?: { force?: boolean; limit?: number }) => Promise<void>
  // 会话页由路由决定显示哪个会话：打开请求交给路由，由 URL 变化再回写活动会话，
  // 避免目录与地址栏各自选中不同的会话。
  openRoutedSession?: (sessionId: string) => void
}

export function useSessionSelection({
  setActiveId,
  loadSessionMessages,
  openRoutedSession,
}: SessionSelectionOptions) {
  const openRoutedSessionRef = useRef(openRoutedSession)
  openRoutedSessionRef.current = openRoutedSession

  const openSession = useCallback(
    (sessionId: string) => {
      if (sessionId && openRoutedSessionRef.current) openRoutedSessionRef.current(sessionId)
      else setActiveId(sessionId)
      return true
    },
    [setActiveId],
  )

  // 其他页面（命令面板、通知、标记）发来的打开请求：带消息定位时强制重读，让定位条目可见。
  useEffect(() => {
    const selectSession = (event: Event) => {
      const detail = (event as CustomEvent<SessionOpenRequest & { id?: string }>).detail
      const sessionId = detail?.sessionId || detail?.id
      if (!sessionId) return
      localStorage.removeItem(STORAGE_KEYS.sessionOpenRequest)
      openSession(sessionId)
      if (detail?.targetEntryId)
        void loadSessionMessages(sessionId, { force: true, limit: FOCUS_MESSAGE_PAGE_SIZE })
    }
    window.addEventListener(SESSION_SELECTED_EVENT, selectSession)
    return () => window.removeEventListener(SESSION_SELECTED_EVENT, selectSession)
  }, [loadSessionMessages, openSession])

  // 会话被删除：上下文页（检查器的阅读位置、预览地址）与未发送草稿一起清掉，不留给同 id 的新会话。
  useEffect(
    () =>
      subscribeSessionDeletionUpdates(window, ({ deletedIds }) => {
        for (const id of deletedIds) {
          useContextPagesStore.getState().forget(id)
          clearComposerDraft(id)
        }
      }),
    [],
  )

  return { openSession }
}
