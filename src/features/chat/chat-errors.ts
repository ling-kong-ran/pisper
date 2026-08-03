export function chatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function isEndedSessionQueueError(error: unknown) {
  return /(?:当前会话已经结束运行|session .*?(?:ended|finished|no longer running))/i.test(
    chatErrorMessage(error),
  )
}
