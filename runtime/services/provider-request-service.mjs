// 通用的会话认证请求：服务只处理认证与传输，接口路径和业务响应由组件负责。
import { isProviderRequestTarget } from '../../shared/provider-request-policy.mjs'
import {
  ComponentFetchProxy,
  checkFetchTarget,
  parseFetchRequest,
} from './custom-ui-fetch-proxy.mjs'
import { CustomUiError } from './custom-ui-error.mjs'
import { lookupProviderAddresses } from '../security/provider-request-dns.mjs'

/** @param {string} code @param {number} [status] @returns {never} */
function fail(code, status = 400) {
  throw new CustomUiError(code, status)
}

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** @param {unknown} input */
export function parseProviderRequest(input) {
  if (
    !record(input) ||
    Object.keys(input).some(
      (key) => !['path', 'method', 'headers', 'body', 'bodyEncoding', 'auth'].includes(key),
    ) ||
    !isProviderRequestTarget(input.path)
  )
    fail('component_provider_request_invalid')
  const auth = input.auth === undefined ? 'provider' : input.auth
  if (
    typeof auth !== 'string' ||
    !['provider', 'bearer', 'x-api-key', 'x-goog-api-key'].includes(auth)
  )
    fail('component_provider_request_invalid')
  const { path, auth: _auth, ...request } = input
  const parsed = parseFetchRequest({ ...request, url: path })
  // 组件不能覆盖 Runtime 认证；附加头仅用于内容协商和内容格式。
  if (
    Object.keys(parsed.headers).some(
      (key) =>
        ![
          'accept',
          'accept-language',
          'content-type',
          'if-none-match',
          'if-modified-since',
        ].includes(key),
    )
  )
    fail('component_provider_request_invalid')
  return { ...parsed, path, auth }
}

/** @template T @param {Promise<T>} promise @param {AbortSignal} signal @returns {Promise<T>} */
async function withAbort(promise, signal) {
  signal.throwIfAborted()
  let abort = () => {}
  const cancelled = new Promise((_, reject) => {
    abort = () => reject(signal.reason)
  })
  signal.addEventListener('abort', abort, { once: true })
  try {
    return await Promise.race([promise, cancelled])
  } finally {
    signal.removeEventListener('abort', abort)
  }
}

/** @param {any} model @param {any} auth @param {string} mode */
function authHeaders(model, auth, mode) {
  /** @type {Record<string, string>} */
  const headers = {}
  const apiKey = typeof auth?.apiKey === 'string' ? auth.apiKey.trim() : ''
  if (apiKey.length > 8192 || /[^\x20-\x7e]/.test(apiKey))
    fail('component_provider_auth_unsupported', 409)
  const protocol = model.api
  if (mode !== 'provider' && !apiKey) fail('component_provider_missing_auth', 409)
  // Azure 的两种 API 都可能共用 openai-completions，必须同时看 provider，不能只按协议猜 Bearer。
  const azure =
    model.provider === 'azure' &&
    ['azure-openai-responses', 'openai-completions'].includes(protocol)
  // Anthropic OAuth 与 GitHub Copilot 使用 Bearer；普通 Anthropic API Key 才使用 x-api-key。
  const anthropicBearer =
    protocol === 'anthropic-messages' &&
    (model.provider === 'github-copilot' || apiKey.includes('sk-ant-oat'))
  const header =
    mode === 'provider'
      ? azure
        ? 'api-key'
        : protocol === 'anthropic-messages'
          ? anthropicBearer
            ? 'bearer'
            : 'x-api-key'
          : protocol === 'google-generative-ai'
            ? 'x-goog-api-key'
            : [
                  'openai-completions',
                  'openai-responses',
                  'openai-codex-responses',
                  'mistral-conversations',
                  'pi-messages',
                ].includes(protocol)
              ? 'bearer'
              : null
      : mode
  if (apiKey && header)
    headers[header === 'bearer' ? 'authorization' : header] =
      header === 'bearer' ? `Bearer ${apiKey}` : apiKey
  // Anthropic SDK 正常请求会自动带版本头；通用传输绕过 SDK，必须补齐并允许配置覆盖或删除。
  if (protocol === 'anthropic-messages') headers['anthropic-version'] ||= '2023-06-01'
  // 已解析的配置头只来源于 ModelRuntime，不接受组件替换；null 沿用“删除默认头”的语义。
  if (mode === 'provider' && record(auth?.headers)) {
    for (const [key, value] of Object.entries(auth.headers)) {
      const name = key.toLowerCase()
      if (
        !/^[a-z0-9!#$%&'*+.^_`|~-]+$/.test(name) ||
        ['host', 'cookie', 'connection', 'content-length', 'transfer-encoding'].includes(name)
      )
        fail('component_provider_auth_unsupported', 409)
      if (value === null) delete headers[name]
      else if (typeof value !== 'string' || value.length > 8192 || /[^\x20-\x7e\t]/.test(value))
        fail('component_provider_auth_unsupported', 409)
      else headers[name] = value
    }
  }
  if (!apiKey && !Object.keys(headers).some((key) => /authorization|api-key|token/.test(key)))
    fail('component_provider_missing_auth', 409)
  if (!header && !Object.keys(headers).length) fail('component_provider_auth_unsupported', 409)
  if (!Object.keys(headers).length) fail('component_provider_missing_auth', 409)
  return {
    headers,
    secrets: [
      apiKey,
      ...Object.entries(headers)
        .filter(([key]) => /authorization|key|token|secret/.test(key))
        .map(([, value]) => value),
    ].filter(Boolean),
  }
}

export class ProviderRequestService {
  /** @param {{ getSession: (id: string) => Promise<any>, getModelRuntime: () => any, transport?: ComponentFetchProxy, timeoutMs?: number }} options */
  constructor({
    getSession,
    getModelRuntime,
    transport = new ComponentFetchProxy({ lookup: lookupProviderAddresses }),
    timeoutMs = 10_000,
  }) {
    this.getSession = getSession
    this.getModelRuntime = getModelRuntime
    this.transport = transport
    this.timeoutMs = timeoutMs
    // 当前会话由宿主单独同步；组件请求只能核对它，不能凭请求体改选另一会话的认证上下文。
    this.selectedSessions = new Map()
  }

  /** @param {unknown} sessionId @param {string} [contextKey] */
  selectSession(sessionId, contextKey = 'local') {
    if (typeof sessionId !== 'string' || !/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$/.test(sessionId))
      fail('component_session_invalid')
    this.selectedSessions.set(contextKey, sessionId)
    return { sessionId }
  }

  /** @param {string} componentId @param {string} sessionId @param {unknown} input @param {AbortSignal} [signal] @param {string} [contextKey] */
  async request(componentId, sessionId, input, signal, contextKey = 'local') {
    const parsed = parseProviderRequest(input)
    if (this.selectedSessions.get(contextKey) !== sessionId)
      fail('component_provider_session_mismatch', 409)
    const release = this.transport.limiter.acquire(componentId)
    const timeout = AbortSignal.timeout(this.timeoutMs)
    const active = signal ? AbortSignal.any([signal, timeout]) : timeout
    try {
      active.throwIfAborted()
      const session = await withAbort(this.getSession(sessionId), active)
      const model = session?.session?.model
      if (!model) fail('component_provider_session_unavailable', 409)
      let base
      try {
        base = new URL(model.baseUrl)
      } catch {
        fail('component_provider_auth_unsupported', 409)
      }
      // 先校验配置地址，再解析凭据；组件始终只能选择该 origin 下已声明的路径。
      checkFetchTarget(base.toString(), [base.hostname])
      active.throwIfAborted()
      const resolution = await withAbort(
        this.getModelRuntime().getAuth(model, { signal: active }),
        active,
      )
      const auth = resolution?.auth
      if (auth?.baseUrl) {
        let override
        try {
          override = new URL(auth.baseUrl)
        } catch {
          fail('component_provider_auth_unsupported', 409)
        }
        // OAuth/动态网关改变 origin 时，不能拿默认服务的路径猜测目标。
        if (override.origin !== base.origin) fail('component_provider_origin_denied', 403)
        base = override
      }
      const target = checkFetchTarget(new URL(parsed.path, base.origin).toString(), [base.hostname])
      const resolved = authHeaders(model, auth, parsed.auth)
      const response = await withAbort(
        this.transport.sendAuthenticated(
          {
            url: target,
            method: parsed.method,
            body: parsed.body,
            headers: { ...parsed.headers, ...resolved.headers },
          },
          active,
        ),
        active,
      )
      // 上游可能回显请求头；拒绝带原始认证值的响应，不把凭据通过正文或响应头绕回组件。
      const content =
        response.bodyEncoding === 'base64'
          ? Buffer.from(response.body, 'base64').toString('utf8')
          : response.body
      const exposed = content + JSON.stringify({ ...response, body: '' })
      if (resolved.secrets.some((secret) => exposed.includes(secret)))
        fail('component_provider_sensitive_response', 502)
      return response
    } catch (error) {
      if (active.aborted)
        fail(
          timeout.aborted ? 'component_provider_timeout' : 'component_request_aborted',
          timeout.aborted ? 504 : 499,
        )
      if (error instanceof CustomUiError) throw error
      fail('component_provider_request_failed', 502)
    } finally {
      release()
    }
  }
}
