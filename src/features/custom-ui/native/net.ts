// ctx.net.fetch（规格 5.8、计划 §4.2）：经 Runtime 代理（POST …/fetch），由代理做主机白名单与 SSRF 防护。
// 这里只做形状转换：请求体字符串按 utf8、二进制按 base64 传；响应按 bodyEncoding 还原成
// {ok, status, headers, url, redirected, text(), json(), arrayBuffer()}。代理拒绝时抛带 code 的错误。
import type {
  ComponentFetchRequest,
  ComponentFetchResult,
} from '@/features/custom-ui/custom-ui-api'

export type FetchInit = {
  method?: string
  headers?: Record<string, string> | Headers | [string, string][]
  body?: string | ArrayBuffer | ArrayBufferView | null
  signal?: AbortSignal
}

export type ComponentFetchResponse = {
  ok: boolean
  status: number
  statusText: string
  headers: Record<string, string>
  url: string
  redirected: boolean
  text(): Promise<string>
  json<T = unknown>(): Promise<T>
  arrayBuffer(): Promise<ArrayBuffer>
}

/** 组件请求（网络代理、存储）被拒绝时的错误：403 主机未声明、429 限流、504 超时、413 配额等；code 是 Runtime 的错误码。 */
export class ComponentRequestError extends Error {
  code: string
  constructor(code: string, message = code) {
    super(message)
    this.name = 'ComponentRequestError'
    this.code = code
  }
}

const CHUNK = 0x8000

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  for (let index = 0; index < bytes.length; index += CHUNK)
    binary += String.fromCharCode(...bytes.subarray(index, index + CHUNK))
  return btoa(binary)
}

export function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value)
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes
}

function normalizeHeaders(headers: FetchInit['headers']): Record<string, string> | undefined {
  if (!headers) return undefined
  const result: Record<string, string> = {}
  const entries =
    typeof Headers !== 'undefined' && headers instanceof Headers
      ? [...headers.entries()]
      : Array.isArray(headers)
        ? headers
        : Object.entries(headers)
  for (const [key, value] of entries) {
    if (typeof key !== 'string' || typeof value !== 'string')
      throw new ComponentRequestError('component_fetch_invalid', 'headers must be strings')
    result[key.toLowerCase()] = value
  }
  return result
}

/** 把 ctx.net.fetch 的参数转成代理请求体；不合法时抛 ComponentRequestError（与代理同一个错误码）。 */
export function buildFetchRequest(url: unknown, init: FetchInit = {}): ComponentFetchRequest {
  if (typeof url !== 'string' || !url)
    throw new ComponentRequestError('component_fetch_invalid', 'url must be a string')
  const request: ComponentFetchRequest = { url }
  if (init.method !== undefined) {
    if (typeof init.method !== 'string')
      throw new ComponentRequestError('component_fetch_invalid', 'method must be a string')
    request.method = init.method.toUpperCase()
  }
  const headers = normalizeHeaders(init.headers)
  if (headers) request.headers = headers
  const body = init.body
  if (body === undefined || body === null) return request
  if (typeof body === 'string') {
    request.body = body
    request.bodyEncoding = 'utf8'
  } else if (body instanceof ArrayBuffer) {
    request.body = bytesToBase64(new Uint8Array(body))
    request.bodyEncoding = 'base64'
  } else if (ArrayBuffer.isView(body)) {
    request.body = bytesToBase64(new Uint8Array(body.buffer, body.byteOffset, body.byteLength))
    request.bodyEncoding = 'base64'
  } else throw new ComponentRequestError('component_fetch_invalid', 'unsupported body type')
  return request
}

export function createFetchResponse(result: ComponentFetchResult): ComponentFetchResponse {
  const bytes = () =>
    result.bodyEncoding === 'base64'
      ? base64ToBytes(result.body)
      : new TextEncoder().encode(result.body)
  const text = () =>
    result.bodyEncoding === 'utf8' ? result.body : new TextDecoder().decode(bytes())
  return Object.freeze({
    ok: result.status >= 200 && result.status < 300,
    status: result.status,
    statusText: result.statusText,
    headers: Object.freeze({ ...result.headers }),
    url: result.url,
    redirected: result.redirected,
    text: async () => text(),
    json: async <T>() => JSON.parse(text()) as T,
    arrayBuffer: async () => {
      const view = bytes()
      return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength) as ArrayBuffer
    },
  })
}

/** 合并组件自己的 signal 与 ctx.signal（卸载 / 热重载时中止）；返回释放监听的函数。 */
export function linkSignals(signals: readonly (AbortSignal | undefined)[]) {
  const controller = new AbortController()
  const cleanups: (() => void)[] = []
  for (const signal of signals) {
    if (!signal) continue
    if (signal.aborted) {
      controller.abort(signal.reason)
      break
    }
    const abort = () => controller.abort(signal.reason)
    signal.addEventListener('abort', abort, { once: true })
    cleanups.push(() => signal.removeEventListener('abort', abort))
  }
  return {
    signal: controller.signal,
    release: () => {
      for (const cleanup of cleanups) cleanup()
    },
  }
}
