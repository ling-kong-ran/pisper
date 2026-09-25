import { remoteFailure } from './decision-errors.mjs'

export const MAX_DECISION_RESPONSE_BYTES = 1024 * 1024
const DEFAULT_TIMEOUT_MS = 120_000

/**
 * 先在单个流上限量读取，再交给 SDK；避免其 clone 缓冲无界增长及旧 Node 的 tee 取消问题。
 * @param {Response} response
 * @param {AbortSignal} signal
 */
async function bufferResponse(response, signal) {
  const headers = new Headers(response.headers)
  // SDK 0.6.0 会把空 Retry-After 当成零，空正文错误也必须保留正常退避。
  for (const key of ['retry-after', 'retry-after-ms']) {
    if (headers.has(key) && !headers.get(key)?.trim()) headers.delete(key)
  }
  const reader = response.body?.getReader()
  if (!reader) return new Response(null, { status: response.status, headers })
  const cancel = () => {
    void reader.cancel().catch(() => {})
  }
  signal.addEventListener('abort', cancel, { once: true })
  let complete = false
  try {
    signal.throwIfAborted()
    const declared = Number(response.headers.get('content-length'))
    if (declared > MAX_DECISION_RESPONSE_BYTES)
      throw remoteFailure('bad_response', { statusCode: 502 })
    let size = 0
    const chunks = []
    for (;;) {
      const { done, value } = await reader.read()
      signal.throwIfAborted()
      if (done) break
      size += value.byteLength
      if (size > MAX_DECISION_RESPONSE_BYTES)
        throw remoteFailure('bad_response', { statusCode: 502 })
      chunks.push(value)
    }
    complete = true
    headers.delete('content-encoding')
    headers.delete('content-length')
    return new Response(Buffer.concat(chunks), { status: response.status, headers })
  } finally {
    signal.removeEventListener('abort', cancel)
    if (!complete) cancel()
    reader.releaseLock()
  }
}

/**
 * SDK 拥有序列化、请求重试和退避；本适配器只负责网关地址、总时限、响应上限与脱敏错误。
 * @param {{ endpoint: string, apiKey: string, modelId: string, state: string, questions: import('@typesafe-ai/sdk').Questions }} input
 * @param {{ signal?: AbortSignal, fetchImpl?: typeof fetch, timeoutMs?: number }} [options]
 * @returns {Promise<unknown>}
 */
export async function requestDecisions(
  input,
  { signal, fetchImpl = fetch, timeoutMs = DEFAULT_TIMEOUT_MS } = {},
) {
  if (signal?.aborted) throw remoteFailure('aborted')
  if (!Number.isInteger(timeoutMs) || timeoutMs <= 0 || timeoutMs > 2_147_483_647)
    throw remoteFailure('invalid')
  const controller = new AbortController()
  const abort = () => controller.abort(signal?.reason)
  signal?.addEventListener('abort', abort, { once: true })
  let timedOut = false
  /** @type {Error | null} */
  let responseFailure = null
  const timer = setTimeout(() => {
    timedOut = true
    controller.abort()
  }, timeoutMs)
  try {
    const { TypeSafeClient, APIError, APITimeoutError, APIUserAbortError, APIConnectionError } =
      await import('@typesafe-ai/sdk')
    if (controller.signal.aborted) throw remoteFailure(signal?.aborted ? 'aborted' : 'timeout')
    const client = new TypeSafeClient({
      apiKey: input.apiKey,
      baseURL: new URL(input.endpoint).origin,
      defaultModel: input.modelId,
      logLevel: 'off',
      timeout: timeoutMs,
      retry: { maxRetries: 3, backoffMaxMs: 8000 },
      fetch: async (_sdkUrl, init) => {
        // 自定义 fetch 是 SDK 的公开扩展点：三种网关仅端点不同，不复制一套重试实现。
        const response = await fetchImpl(input.endpoint, { ...init, redirect: 'error' })
        try {
          return await bufferResponse(response, init?.signal || controller.signal)
        } catch (error) {
          if (
            error &&
            typeof error === 'object' &&
            'code' in error &&
            error.code === 'bad_response'
          ) {
            responseFailure = error instanceof Error ? error : null
            controller.abort()
          }
          throw error
        }
      },
    })
    try {
      return await client.systemOne(
        { model: input.modelId, state: input.state, questions: input.questions },
        { signal: controller.signal },
      )
    } catch (error) {
      if (responseFailure) throw responseFailure
      if (signal?.aborted) throw remoteFailure('aborted')
      if (timedOut || error instanceof APITimeoutError)
        throw remoteFailure('timeout', { statusCode: 502, retryable: true })
      if (error instanceof APIUserAbortError) throw remoteFailure('aborted')
      if (error instanceof APIError) {
        const status = error.status
        if ([401, 402, 403].includes(status)) throw remoteFailure('auth', { statusCode: 401 })
        if (status === 413) throw remoteFailure('state_too_large', { statusCode: 413 })
        if ([400, 422].includes(status)) throw remoteFailure('invalid')
        if (status === 429)
          throw remoteFailure('rate_limited', { statusCode: 429, retryable: true })
        if (status >= 500) throw remoteFailure('overloaded', { statusCode: 502, retryable: true })
      }
      if (error instanceof APIConnectionError)
        throw remoteFailure('network', { statusCode: 502, retryable: true })
      // SDK 错误可能附带原始正文或含密钥的 cause；只允许稳定错误离开本边界。
      throw remoteFailure('bad_response', { statusCode: 502 })
    }
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener('abort', abort)
  }
}
