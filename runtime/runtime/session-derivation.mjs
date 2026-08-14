const INCOMPLETE_STOP_REASONS = new Set(['toolUse', 'error', 'aborted'])

export function isCompletedTurnBoundaryMessage(message) {
  if (message?.role !== 'assistant' || message.errorMessage) return false
  const toolCalls = Array.isArray(message.content)
    ? message.content.filter((part) => part?.type === 'toolCall')
    : []
  if (message.stopReason) return !INCOMPLETE_STOP_REASONS.has(message.stopReason)
  return toolCalls.length === 0
}
