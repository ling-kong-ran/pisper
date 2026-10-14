import { i18n, storedLanguage, translateText, isSupportedLanguage } from '@/app/i18n'
import { ApiError, type ApiErrorPayload } from '@/lib/api-error'

export function invalidResponseError(status?: number) {
  return new ApiError(
    translateText(
      'common:api.invalidResponse',
      isSupportedLanguage(i18n.language) ? i18n.language : storedLanguage(),
    ),
    { status, kind: 'protocol', data: { code: 'INVALID_RESPONSE' } },
  )
}

function errorPayload(text: string): ApiErrorPayload | undefined {
  if (!text) return undefined
  try {
    const parsed: unknown = JSON.parse(text)
    if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed)) {
      // 保留错误码等兼容字段，但对象/数组形式的 error 不能冒充可展示的消息。
      return {
        ...parsed,
        error: 'error' in parsed && typeof parsed.error === 'string' ? parsed.error : undefined,
      }
    }
    return { error: typeof parsed === 'string' ? parsed : text, body: parsed }
  } catch {
    return { error: text }
  }
}

export async function readHttpError(response: Response): Promise<ApiError> {
  const data = errorPayload(await response.text())
  return new ApiError(data?.error || response.statusText || `HTTP ${response.status}`, {
    status: response.status,
    data,
    kind: 'http',
  })
}

export async function readJsonResponse(response: Response): Promise<unknown> {
  if (response.status === 204) return undefined
  const text = await response.text()
  if (!text) return undefined
  try {
    return JSON.parse(text)
  } catch {
    // 不把代理错误页作为业务成功值，也不将可能含凭据的响应正文写进诊断。
    throw invalidResponseError(response.status)
  }
}
