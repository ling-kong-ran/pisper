type StreamingState = { loaded?: boolean; streaming?: boolean }
type StreamingSummary = { streaming?: boolean }

// 已加载的会话状态拥有本轮终态；稍晚返回的目录摘要不能重新点亮“停止”。
// 恢复阶段尚无正文/实时快照时仍允许摘要提示后台运行，本地新一轮始终优先。
export function resolveSessionStreaming(
  state: StreamingState | null | undefined,
  summary?: StreamingSummary | null,
): boolean {
  if (state?.streaming) return true
  if (state?.loaded) return false
  return Boolean(summary?.streaming)
}
