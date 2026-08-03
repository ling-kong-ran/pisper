type ApiErrorPayload = {
  error?: string
  [key: string]: unknown
}

export type HttpRequestOptions = Omit<RequestInit, 'body' | 'signal'> & {
  body?: unknown
  data?: unknown
  signal?: AbortSignal
  timeout?: number
}

export const DEFAULT_HTTP_TIMEOUT_MS = 30_000

export class ApiError extends Error {
  status?: number
  data?: ApiErrorPayload

  constructor(message: string, options: { status?: number; data?: ApiErrorPayload } = {}) {
    super(message)
    this.name = 'ApiError'
    this.status = options.status
    this.data = options.data
  }
}

function errorPayload(text: string): ApiErrorPayload | undefined {
  if (!text) return undefined
  try {
    const parsed: unknown = JSON.parse(text)
    if (typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed))
      return parsed as ApiErrorPayload
    return {
      error: typeof parsed === 'string' ? parsed : text,
      body: parsed,
    }
  } catch {
    return { error: text }
  }
}

function errorMessage(error: unknown, fallback: string) {
  if (error instanceof Error && error.message) return error.message
  if (typeof error === 'string' && error) return error
  return fallback
}

export async function requestJson<T = unknown>(
  path: string,
  options: HttpRequestOptions = {},
): Promise<T> {
  const {
    body,
    data,
    headers: inputHeaders,
    signal: externalSignal,
    timeout = DEFAULT_HTTP_TIMEOUT_MS,
    ...requestOptions
  } = options
  const payload = data !== undefined ? data : body
  const headers = new Headers(inputHeaders)
  let requestBody: BodyInit | undefined
  if (payload !== undefined) {
    requestBody = typeof payload === 'string' ? payload : JSON.stringify(payload)
    if (!headers.has('Content-Type')) headers.set('Content-Type', 'application/json')
  }

  const controller = new AbortController()
  let timedOut = false
  const abortFromCaller = () => controller.abort(externalSignal?.reason)
  if (externalSignal?.aborted) abortFromCaller()
  else externalSignal?.addEventListener('abort', abortFromCaller, { once: true })
  const timeoutId =
    timeout > 0
      ? setTimeout(() => {
          timedOut = true
          controller.abort(new DOMException('Request timed out', 'TimeoutError'))
        }, timeout)
      : undefined

  try {
    const response = await fetch(path, {
      ...requestOptions,
      body: requestBody,
      headers,
      signal: controller.signal,
    })
    if (response.status === 204) return undefined as T

    const text = await response.text()
    if (!response.ok) {
      const normalized = errorPayload(text)
      throw new ApiError(
        normalized?.error || response.statusText || `请求失败 (${response.status})`,
        {
          status: response.status,
          data: normalized,
        },
      )
    }
    if (!text) return undefined as T
    try {
      return JSON.parse(text) as T
    } catch {
      return text as T
    }
  } catch (error) {
    if (error instanceof ApiError) throw error
    if (timedOut) throw new ApiError(`请求超时 (${timeout}ms)`)
    if (externalSignal?.aborted)
      throw new ApiError(errorMessage(externalSignal.reason, '请求已取消'))
    throw new ApiError(errorMessage(error, '请求失败 (network)'))
  } finally {
    if (timeoutId !== undefined) clearTimeout(timeoutId)
    externalSignal?.removeEventListener('abort', abortFromCaller)
  }
}
