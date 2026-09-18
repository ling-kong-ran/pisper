import { useMemo } from 'react'
import { runActivityStatusLabel } from '@/features/chat/AgentRunActivity'
import type { FocusSessionProps } from '@/features/chat/focus-session-props'
import type { Translate } from '@/features/chat/run-activity-presentation'

type FocusSessionStatus = Pick<
  FocusSessionProps,
  | 'streaming'
  | 'messages'
  | 'currentActivity'
  | 'thinkingText'
  | 'compaction'
  | 'error'
  | 'runStopped'
  | 'runNotice'
  | 'lastActivityAt'
>

// 状态胶囊复用活动区的状态推导，并保留逐字段缓存，避免输入草稿时重复计算。
export function useFocusSessionStatusLabel(
  {
    streaming,
    messages,
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
  return useMemo(
    () =>
      runActivityStatusLabel(
        {
          streaming,
          text: messages.at(-1)?.role === 'agent' ? messages.at(-1)?.text || '' : '',
          currentActivity,
          thinkingText,
          compaction,
          error,
          stopped: runStopped,
          notice: runNotice,
          lastActivityAt,
        },
        t,
      ),
    [
      streaming,
      messages,
      currentActivity,
      thinkingText,
      compaction,
      error,
      runStopped,
      runNotice,
      lastActivityAt,
      t,
    ],
  )
}
