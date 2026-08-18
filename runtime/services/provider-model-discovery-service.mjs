// Provider 模型发现：调用各 Provider 的 /models 端点拉取远程模型列表，
// 自动推断模型能力（kind/reasoning/上下文窗口），错误信息先脱敏再返回。
import { redactSecretText } from '../security/secret-redaction.mjs'
import { inferModelKind } from './visual-generation/index.mjs'

const DEFAULT_TIMEOUT_MS = 12_000
const DEFAULT_MAX_RESPONSE_BYTES = 2 * 1024 * 1024

// 由 Base URL 构造 /models 端点 URL。
function modelsUrl(baseUrl) {
  let url
  try {
    url = new URL(String(baseUrl || '').trim())
  } catch {
    throw new Error('Provider Base URL 无效。')
  }
  if (!['http:', 'https:'].includes(url.protocol))
    throw new Error('Provider Base URL 仅支持 HTTP 或 HTTPS。')
  url.search = ''
  url.hash = ''
  const path = url.pathname.replace(/\/+$/, '')
  url.pathname = /\/models$/i.test(path) ? path : `${path}/models`
  return url
}

function apiKeyValue(value) {
  return String(value || '')
    .trim()
    .replace(/^Bearer\s+/i, '')
}

function requestHeaders(api, apiKey, organization, extraHeaders) {
  const headers = { Accept: 'application/json' }
  for (const [name, value] of Object.entries(extraHeaders || {})) {
    if (!/^(?:host|content-length)$/i.test(name) && typeof value === 'string' && value.trim())
      headers[name] = value.trim()
  }
  if (api === 'anthropic-messages') {
    if (apiKey) headers['x-api-key'] = apiKeyValue(apiKey)
    headers['anthropic-version'] = '2023-06-01'
  } else if (api === 'google-generative-ai') {
    if (apiKey) headers['x-goog-api-key'] = apiKeyValue(apiKey)
  } else {
    if (apiKey) headers.Authorization = /^Bearer\s+/i.test(apiKey) ? apiKey : `Bearer ${apiKey}`
    if (organization) headers['OpenAI-Organization'] = String(organization).trim()
  }
  return headers
}

async function readJsonLimited(response, maxBytes) {
  const declaredSize = Number(response.headers?.get?.('content-length') || 0)
  if (declaredSize > maxBytes) throw new Error('Provider 返回的模型列表过大。')
  if (!response.body?.getReader) {
    const text = await response.text()
    if (Buffer.byteLength(text, 'utf8') > maxBytes) throw new Error('Provider 返回的模型列表过大。')
    return { payload: text ? JSON.parse(text) : {}, bytes: Buffer.byteLength(text, 'utf8') }
  }

  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let size = 0
  let text = ''
  while (true) {
    const { value, done } = await reader.read()
    if (done) break
    size += value.byteLength
    if (size > maxBytes) {
      await reader.cancel().catch(() => {})
      throw new Error('Provider 返回的模型列表过大。')
    }
    text += decoder.decode(value, { stream: true })
  }
  text += decoder.decode()
  return { payload: text ? JSON.parse(text) : {}, bytes: size }
}

function errorMessage(payload, status) {
  const raw = payload?.error?.message || payload?.error || payload?.message || payload?.detail || ''
  const detail = typeof raw === 'string' ? redactSecretText(raw).slice(0, 240) : ''
  return detail ? `获取模型失败 (${status})：${detail}` : `获取模型失败 (${status})。`
}

function candidateFrom(item, api) {
  if (typeof item === 'string') return { id: item, name: item }
  if (!item || typeof item !== 'object') return null
  const rawId = item.id || item.model_id || item.model || item.slug || item.name
  if (!rawId) return null
  const normalizeName = (value) =>
    api === 'google-generative-ai' ? String(value).replace(/^models\//, '') : String(value)
  const id = normalizeName(rawId).trim()
  if (!id) return null
  const name = normalizeName(item.display_name || item.displayName || item.name || id).trim() || id
  return {
    id,
    name,
    // 不再按 ID 推断用途：发现的模型一律先按对话模型列出，
    // 实际类型由用户在添加时显式选择（详见 inferModelKind）。
    kind: inferModelKind(id, 'auto'),
    ...(Array.isArray(item.supportedGenerationMethods)
      ? { supportedGenerationMethods: item.supportedGenerationMethods.map(String) }
      : {}),
  }
}

function responseItems(payload, api) {
  if (api === 'google-generative-ai') return Array.isArray(payload?.models) ? payload.models : []
  if (Array.isArray(payload?.data)) return payload.data
  if (Array.isArray(payload?.models)) return payload.models
  return Array.isArray(payload) ? payload : []
}

function nextPageUrl(payload, api, currentUrl) {
  const next = new URL(currentUrl)
  if (api === 'google-generative-ai' && payload?.nextPageToken) {
    next.searchParams.set('pageToken', String(payload.nextPageToken))
    return next
  }
  if (payload?.has_more && payload?.last_id) {
    next.searchParams.set('after_id', String(payload.last_id))
    return next
  }
  return null
}

export class ProviderModelDiscoveryService {
  constructor({
    fetchImpl = globalThis.fetch,
    timeoutMs = DEFAULT_TIMEOUT_MS,
    maxResponseBytes = DEFAULT_MAX_RESPONSE_BYTES,
  } = {}) {
    this.fetch = fetchImpl
    this.timeoutMs = timeoutMs
    this.maxResponseBytes = maxResponseBytes
    this.controllers = new Set()
  }

  abort() {
    for (const controller of this.controllers) controller.abort()
  }

  async discover({ api, baseUrl, apiKey, organization, headers } = {}) {
    const protocol = String(api || 'openai-responses').trim()
    if (
      ![
        'openai-responses',
        'openai-completions',
        'anthropic-messages',
        'google-generative-ai',
      ].includes(protocol)
    ) {
      throw new Error('当前 API 协议不支持自动获取模型。')
    }
    const url = modelsUrl(baseUrl)
    const controller = new AbortController()
    this.controllers.add(controller)
    const timer = setTimeout(() => controller.abort(), this.timeoutMs)
    try {
      const unique = new Map()
      const seenPages = new Set()
      let pageUrl = url
      let totalBytes = 0
      for (let page = 0; page < 50 && pageUrl; page += 1) {
        const pageKey = String(pageUrl)
        if (seenPages.has(pageKey)) throw new Error('Provider 返回了重复的模型分页游标。')
        seenPages.add(pageKey)
        const response = await this.fetch(pageUrl, {
          method: 'GET',
          headers: requestHeaders(protocol, String(apiKey || '').trim(), organization, headers),
          signal: controller.signal,
        })
        let pageResult
        try {
          pageResult = await readJsonLimited(response, this.maxResponseBytes - totalBytes)
        } catch (error) {
          if (!response.ok) throw new Error(`获取模型失败 (${response.status})。`)
          if (error instanceof SyntaxError) throw new Error('Provider 返回了无效的模型列表。')
          throw error
        }
        const { payload, bytes } = pageResult
        totalBytes += bytes
        if (!response.ok) throw new Error(errorMessage(payload, response.status))
        for (const item of responseItems(payload, protocol)) {
          const candidate = candidateFrom(item, protocol)
          if (candidate && !unique.has(candidate.id)) unique.set(candidate.id, candidate)
        }
        pageUrl = nextPageUrl(payload, protocol, pageUrl)
        if (page === 49 && pageUrl) throw new Error('Provider 返回的模型分页过多。')
      }
      const models = [...unique.values()].sort((left, right) =>
        left.id.localeCompare(right.id, undefined, { numeric: true, sensitivity: 'base' }),
      )
      if (!models.length) throw new Error('Provider 没有返回可用的模型 ID。')
      return { models, count: models.length }
    } catch (error) {
      if (error?.name === 'AbortError')
        throw new Error('获取模型超时，请检查 Provider 地址或网络。')
      throw error
    } finally {
      clearTimeout(timer)
      this.controllers.delete(controller)
    }
  }
}
