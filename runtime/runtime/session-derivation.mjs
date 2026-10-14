// 判断一条助手消息是否构成“已完成的回合边界”：
// 正常结束（stop/length 等非 toolUse/error/aborted 原因）或没有工具调用即视为完成。
const INCOMPLETE_STOP_REASONS = new Set(['toolUse', 'error', 'aborted'])

export function isCompletedTurnBoundaryMessage(message) {
  if (message?.role !== 'assistant' || message.errorMessage) return false
  const toolCalls = Array.isArray(message.content)
    ? message.content.filter((part) => part?.type === 'toolCall')
    : []
  if (message.stopReason) return !INCOMPLETE_STOP_REASONS.has(message.stopReason)
  return toolCalls.length === 0
}
