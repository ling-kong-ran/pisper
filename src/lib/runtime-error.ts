import { i18n, isSupportedLanguage, storedLanguage, translateText } from '@/app/i18n'

// Runtime 的错误带 { code, params }（见 shared/errors.mjs）：按界面语言翻译；
// 不认识的码、没有码或不是 PisperError 时用 Runtime 给的 message，不丢信息。
const CODES = new Set([
  'invalid',
  'too_large',
  'not_found',
  'exists',
  'busy',
  'limit',
  'unavailable',
  'unsupported',
  'forbidden',
  'cancelled',
  'timeout',
  'failed',
])

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

export function localizedRuntimeError(payload: unknown, fallback: string): string {
  const data = record(payload)
  const code = typeof data.code === 'string' ? data.code : ''
  // 只翻译 PisperError（总带 params 对象）；旧接口自有的同名码（如 not_found）保留它们自己的 message。
  if (!CODES.has(code) || !data.params || typeof data.params !== 'object') return fallback
  const params = record(data.params)
  const language = isSupportedLanguage(i18n.language) ? i18n.language : storedLanguage()
  const subjectKey =
    typeof params.subject === 'string' && params.subject ? params.subject : 'request'
  const subject = i18n.exists(`errors:subject.${subjectKey}`)
    ? translateText(`errors:subject.${subjectKey}`, language)
    : subjectKey
  const message = translateText(`errors:code.${code}`, language, { subject })
  const detail = typeof params.detail === 'string' ? params.detail.trim() : ''
  return detail
    ? translateText('errors:withDetail', language, { message, detail })
    : translateText('errors:plain', language, { message })
}
