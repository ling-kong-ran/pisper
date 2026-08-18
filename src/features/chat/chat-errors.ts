// 聊天错误归一化：把任意异常转成可展示文案，并识别
// “会话已结束仍排队”等特殊错误以便调用方决定是否重试。
export function chatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function isEndedSessionQueueError(error: unknown) {
  return /(?:当前会话已经结束运行|session .*?(?:ended|finished|no longer running))/i.test(
    chatErrorMessage(error),
  )
}
