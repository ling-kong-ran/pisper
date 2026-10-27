import { useEffect, useMemo, useState } from 'react'
import { runActivityStatusLabel } from '@/features/chat/AgentRunActivity'
import type { FocusSessionProps } from '@/features/chat/focus-session-props'
import type { Translate } from '@/features/chat/run-activity-presentation'

type FocusSessionStatus = Pick<
  FocusSessionProps,
  | 'streaming'
  | 'messages'
  | 'tools'
  | 'currentActivity'
  | 'thinkingText'
  | 'compaction'
  | 'error'
  | 'runStopped'
  | 'runNotice'
  | 'lastActivityAt'
>

// 状态胶囊自己计时，避免整页转录随秒针重渲染。
// 并行终端里，卡住的那条要在没有新 SSE 时也能从“思考中”切到等待。
function useRunActivityClock(streaming?: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!streaming) return undefined
    setNow(Date.now())
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [streaming])
  return now
}

// 状态胶囊复用活动区的状态推导，并保留逐字段缓存，避免输入草稿时重复计算。
export function useFocusSessionStatusLabel(
  {
    streaming,
    messages,
    tools,
    currentActivity,
    thinkingText,
    compaction,
    error,
    runStopped,
    runNotice,
    lastActivityAt,
  }: FocusSessionStatus,
  t: Translate,
) {
  const now = useRunActivityClock(streaming)
  return useMemo(
    () =>
      runActivityStatusLabel(
        {
          streaming,
          text: messages.at(-1)?.role === 'agent' ? messages.at(-1)?.text || '' : '',
          tools,
          currentActivity,
          thinkingText,
          compaction,
          error,
          stopped: runStopped,
          notice: runNotice,
          lastActivityAt,
          now,
        },
        t,
      ),
    [
      streaming,
      messages,
      tools,
      currentActivity,
      thinkingText,
      compaction,
      error,
      runStopped,
      runNotice,
      lastActivityAt,
      now,
      t,
    ],
  )
}
