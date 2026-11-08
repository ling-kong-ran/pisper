import { ApiError } from '@/lib/api-error'

// 聊天错误归一化：把任意异常转成可展示文案，并识别
// “会话已结束仍排队”等特殊错误以便调用方决定是否重试。
export function chatErrorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

// 旧 Runtime 回中文，新 Runtime 回英文 message（界面文案已按语言翻译，所以同时看原始 message）。
const ENDED_SESSION_PATTERN =
  /(?:当前会话已经结束运行|session .*?(?:ended|finished|no longer running))/i

export function isEndedSessionQueueError(error: unknown) {
  const raw =
    error instanceof ApiError && typeof error.data?.error === 'string' ? error.data.error : ''
  return ENDED_SESSION_PATTERN.test(raw) || ENDED_SESSION_PATTERN.test(chatErrorMessage(error))
}
