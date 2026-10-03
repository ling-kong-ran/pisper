import { useEffect, useLayoutEffect, useRef } from 'react'
import { chatApi } from '@/features/chat/api/chat-api'
import {
  didCompleteSessionContextRun,
  shouldRevealSessionContext,
  type SessionContextRun,
} from '@/features/chat/model/session-context-layout'

type SessionContextAutoRevealOptions = SessionContextRun & {
  enabled: boolean
  open: boolean
  onReveal: () => void
}

// 独立侧栏与画布内上下文共用自动展示规则；请求只属于当前可见会话的已完成轮次。
export function useSessionContextAutoReveal({
  sessionId,
  streaming,
  completed,
  runStartedAt,
  enabled,
  open,
  onReveal,
}: SessionContextAutoRevealOptions) {
  const previousRunRef = useRef<SessionContextRun | null>(null)
  const revealRef = useRef(onReveal)
  useLayoutEffect(() => {
    revealRef.current = onReveal
  }, [onReveal])

  useEffect(() => {
    const previous = previousRunRef.current
    const current = { sessionId, streaming, completed, runStartedAt }
    previousRunRef.current = current
    // 已打开的面板保留用户选中的页面，也无需为自动展示重复读取文件。
    if (!enabled || open || !didCompleteSessionContextRun(previous, current)) return
    const controller = new AbortController()
    let active = true
    void chatApi
      .getSessionFileChanges(sessionId, { signal: controller.signal })
      .then(({ files }) => {
        if (active && shouldRevealSessionContext(previous, current, files)) revealRef.current()
      })
      .catch(() => {
        // 自动检查失败时保留当前布局；用户仍可手动打开面板查看错误或刷新。
      })
    return () => {
      active = false
      controller.abort()
    }
  }, [completed, enabled, open, runStartedAt, sessionId, streaming])
}
