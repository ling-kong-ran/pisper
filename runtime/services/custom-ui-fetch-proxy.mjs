// 组件网络代理：原生组件的 ctx.net.fetch 一律经 POST /api/custom-ui/components/:id/fetch 走这里。
//
// 规则（规格 5.8、计划 §3.7）：
// - 只放行 manifest 声明过的主机，只允许 https、443 端口，不允许 userinfo 与 IP 字面量；
// - 例外是 network.privateOrigins 里逐个声明的私网源（scheme + 主机 + 端口完全一致）：可以是 http、
//   任意端口和私网 / 回环 IP 字面量，但解析结果必须全部落在私网或回环段（链路本地、元数据仍然阻断）；
//   这类请求都带来源标记，Pisper 自己的监听见到就拒绝，组件不能借 localhost 调用应用 API；
// - 先自己解析 DNS、校验全部地址，再只把校验过的 IP 交给连接（防 DNS rebinding）；
// - 手动跟随重定向（≤5 跳），每一跳重新做全部校验，跨主机时去掉组件自带的 Authorization；
// - 10 秒总超时，响应 >2 MiB 立即中断；请求头与响应头都按白名单；
// - 绝不转发应用请求里的 Cookie / Authorization / Origin：代理从空白开始构造请求；
// - 每个组件令牌桶每分钟 30 次、并发 ≤4；统计只记主机、方法、状态码、耗时，不记查询参数与请求体。
import { lookup as dnsLookup } from 'node:dns/promises'
import { request as httpRequest } from 'node:http'
import { request as httpsRequest } from 'node:https'
import { isIP } from 'node:net'
import { toWellFormed } from '../../shared/custom-ui-manifest.mjs'
import { COMPONENT_PROXY_HEADER } from '../security/component-proxy-marker.mjs'
import {
  isBlockedAddress,
  isBlockedHostname,
  isBlockedPrivateHostname,
  isPrivateNetworkAddress,
} from '../security/public-address.mjs'
import { CustomUiError } from './custom-ui-error.mjs'

export const FETCH_MAX_URL_LENGTH = 2048
export const FETCH_TIMEOUT_MS = 10_000
export const FETCH_MAX_RESPONSE_BYTES = 2 * 1024 * 1024
export const FETCH_MAX_REQUEST_BODY_BYTES = 384 * 1024
export const FETCH_MAX_REDIRECTS = 5
export const FETCH_RATE_PER_MINUTE = 30
export const FETCH_MAX_CONCURRENT = 4
const USER_AGENT = 'Pisper-Component/1'
const METHODS = new Set(['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE'])
// 组件可以自己给的请求头；authorization 只能是组件自己的值（负责人决定，规格 5.8）。
const REQUEST_HEADERS = new Set([
  'accept',
  'accept-language',
  'content-type',
  'if-none-match',
  'if-modified-since',
  'authorization',
])
const RESPONSE_HEADERS = new Set([
  'content-type',
  'content-length',
  'etag',
  'last-modified',
  'cache-control',
  'link',
  'retry-after',
  'date',
])
const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308])
const MAX_HEADER_VALUE = 8 * 1024
const MAX_TRACKED_HOSTS = 32

/** @param {unknown} value @returns {value is Record<string, unknown>} */
function isRecord(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

/** @param {string} field @returns {never} */
function invalid(field) {
  throw new CustomUiError('component_fetch_invalid', 400, { field })
}

/**
 * 声明的主机是否覆盖目标主机：精确匹配；`*.example.com` 只匹配恰好多一级的子域（不含 example.com 本身）。
 * @param {readonly string[]} hosts
 * @param {string} hostname
 */
export function hostMatches(hosts, hostname) {
  return hosts.some((pattern) => {
    if (!pattern.startsWith('*.')) return pattern === hostname
    const suffix = pattern.slice(1)
    if (!hostname.endsWith(suffix)) return false
    const label = hostname.slice(0, -suffix.length)
    return label.length > 0 && !label.includes('.')
  })
}

/**
 * 校验一个目标 URL（首跳与每一次重定向都调用）。
 * @param {unknown} raw
 * @param {readonly string[]} hosts
 * @param {string} [field]
 * @returns {URL}
 */
export function checkFetchTarget(raw, hosts, field = 'url') {
  if (typeof raw !== 'string' || !raw || raw.length > FETCH_MAX_URL_LENGTH) invalid(field)
  let url
  try {
    url = new URL(raw)
  } catch {
    return invalid(field)
  }
  if (url.protocol !== 'https:' || url.username || url.password) invalid(field)
  if (url.port && url.port !== '443') invalid(field)
  const hostname = url.hostname.toLowerCase()
  // URL 会把 IPv6 字面量包在方括号里，也会把十进制 / 十六进制 IPv4 规范成点分写法。
  if (!hostname || hostname.startsWith('[') || isIP(hostname) || hostname.endsWith('.'))
    invalid(field)
  if (isBlockedHostname(hostname))
    throw new CustomUiError('component_fetch_address_blocked', 403, { host: hostname })
  if (!hostMatches(hosts, hostname))
    throw new CustomUiError('component_fetch_host_denied', 403, { host: hostname })
  url.hash = ''
  return url
}

/**
 * @typedef {{ hosts: readonly string[], privateOrigins?: readonly string[] }} NetworkPolicy
 * @typedef {{ url: URL, privateTarget: boolean }} FetchTarget
 */

/**
 * 按 manifest 的 network 能力校验目标：命中声明过的私网源时走私网规则，否则按公网规则（checkFetchTarget）。
 * @param {unknown} raw
 * @param {NetworkPolicy} network
 * @param {string} [field]
 * @returns {FetchTarget}
 */
export function resolveFetchTarget(raw, network, field = 'url') {
  const origins = network.privateOrigins || []
  if (origins.length && typeof raw === 'string' && raw && raw.length <= FETCH_MAX_URL_LENGTH) {
    let url = null
    try {
      url = new URL(raw)
    } catch {
      url = null
    }
    if (url && !url.username && !url.password && origins.includes(url.origin)) {
      const hostname = bareHostname(url)
      if (isBlockedPrivateHostname(hostname))
        throw new CustomUiError('component_fetch_address_blocked', 403, { host: hostname })
      url.hash = ''
      return { url, privateTarget: true }
    }
  }
  return { url: checkFetchTarget(raw, network.hosts, field), privateTarget: false }
}

/** URL.hostname 会给 IPv6 字面量带方括号；连接、SNI 与地址校验都要去掉。 @param {URL} url */
function bareHostname(url) {
  const host = url.hostname.toLowerCase()
  return host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host
}

/**
 * 请求体按不可信 unknown 校验：{url, method?, headers?, body?, bodyEncoding?}。
 * @param {unknown} input
 */
export function parseFetchRequest(input) {
  if (!isRecord(input)) invalid('request')
  const allowed = ['url', 'method', 'headers', 'body', 'bodyEncoding']
  const unknown = Object.keys(input).find((key) => !allowed.includes(key))
  if (unknown) invalid(unknown)
  let method = 'GET'
  if (input.method !== undefined) {
    if (typeof input.method !== 'string') invalid('method')
    method = input.method.toUpperCase()
    if (!METHODS.has(method)) invalid('method')
  }
  /** @type {Record<string, string>} */
  const headers = {}
  if (input.headers !== undefined) {
    if (!isRecord(input.headers)) invalid('headers')
    const entries = Object.entries(input.headers)
    if (entries.length > REQUEST_HEADERS.size) invalid('headers')
    for (const [rawName, value] of entries) {
      const name = rawName.toLowerCase()
      if (!REQUEST_HEADERS.has(name)) invalid(`headers.${name}`)
      if (typeof value !== 'string' || value.length > MAX_HEADER_VALUE || !isHeaderValue(value))
        invalid(`headers.${name}`)
      headers[name] = value
    }
  }
  let bodyEncoding = 'utf8'
  if (input.bodyEncoding !== undefined) {
    if (input.bodyEncoding !== 'utf8' && input.bodyEncoding !== 'base64') invalid('bodyEncoding')
    bodyEncoding = input.bodyEncoding
  }
  /** @type {Buffer | null} */
  let body = null
  if (input.body !== undefined && input.body !== null) {
    if (typeof input.body !== 'string' || method === 'GET' || method === 'HEAD') invalid('body')
    if (bodyEncoding === 'base64') {
      if (!/^[A-Za-z0-9+/]*={0,2}$/.test(input.body) || input.body.length % 4 !== 0) invalid('body')
      body = Buffer.from(input.body, 'base64')
    } else body = Buffer.from(toWellFormed(input.body), 'utf8')
    if (body.length > FETCH_MAX_REQUEST_BODY_BYTES) invalid('body')
  }
  return { url: input.url, method, headers, body }
}

/**
 * 请求头值只允许 Latin-1 可见字符与制表符：拒绝 CR/LF 等控制字符（防请求头注入），
 * 也拒绝 Node 会报错或按 Latin-1 截断的宽字符。
 * @param {string} value
 */
function isHeaderValue(value) {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if ((code < 0x20 && code !== 0x09) || code === 0x7f || code > 0xff) return false
  }
  return true
}

/** @param {string} contentType */
function isTextContent(contentType) {
  const type = contentType.split(';')[0].trim().toLowerCase()
  return (
    type.startsWith('text/') ||
    type === 'application/json' ||
    type.endsWith('+json') ||
    type === 'application/xml' ||
    type.endsWith('+xml') ||
    type === 'application/javascript' ||
    type === 'application/x-www-form-urlencoded' ||
    type === 'image/svg+xml'
  )
}

/** @param {import('node:http').IncomingHttpHeaders} headers */
function publicResponseHeaders(headers) {
  /** @type {Record<string, string>} */
  const result = {}
  for (const [name, value] of Object.entries(headers)) {
    if (value === undefined) continue
    if (!RESPONSE_HEADERS.has(name) && !name.startsWith('x-ratelimit-')) continue
    result[name] = toWellFormed(Array.isArray(value) ? value.join(', ') : String(value))
  }
  return result
}

/** 每个组件一个令牌桶 + 并发计数；只在内存里。 */
class ComponentRateLimiter {
  /** @param {{ now: () => number, perMinute: number, concurrent: number }} options */
  constructor({ now, perMinute, concurrent }) {
    this.now = now
    this.perMinute = perMinute
    this.concurrent = concurrent
    /** @type {Map<string, { tokens: number, at: number, active: number }>} */
    this.buckets = new Map()
  }

  /** @param {string} id @returns {() => void} */
  acquire(id) {
    const now = this.now()
    const bucket = this.buckets.get(id) || { tokens: this.perMinute, at: now, active: 0 }
    bucket.tokens = Math.min(
      this.perMinute,
      bucket.tokens + ((now - bucket.at) / 60_000) * this.perMinute,
    )
    bucket.at = now
    if (bucket.tokens < 1 || bucket.active >= this.concurrent) {
      this.buckets.set(id, bucket)
      throw new CustomUiError('component_fetch_rate_limited', 429)
    }
    bucket.tokens -= 1
    bucket.active += 1
    this.buckets.set(id, bucket)
    let released = false
    return () => {
      if (released) return
      released = true
      bucket.active -= 1
    }
  }

  /** @param {string} id */
  forget(id) {
    this.buckets.delete(id)
  }
}

export class ComponentFetchProxy {
  /**
   * @param {{
   *   lookup?: (hostname: string, signal?: AbortSignal) => Promise<{ address: string, family: number }[]>,
   *   isBlockedAddress?: (address: string) => boolean,
   *   isPrivateAddress?: (address: string) => boolean,
   *   request?: typeof httpsRequest,
   *   httpRequest?: typeof httpRequest,
   *   ca?: string | Buffer,
   *   port?: number,
   *   now?: () => number,
   *   timeoutMs?: number,
   *   maxResponseBytes?: number,
   *   perMinute?: number,
   *   concurrent?: number,
   * }} [options]
   *   lookup / isBlockedAddress / isPrivateAddress / ca / port 只供测试注入（本地 TLS 服务）；产品路径一律用默认值。
   *   port 只作用于公网目标；私网源按声明的端口连接。
   */
  constructor({
    lookup = (hostname) => dnsLookup(hostname, { all: true, verbatim: true }),
    isBlockedAddress: blocked = (address) => isBlockedAddress(address),
    isPrivateAddress = (address) => isPrivateNetworkAddress(address),
    request = httpsRequest,
    httpRequest: plainRequest = httpRequest,
    ca,
    port = 443,
    now = Date.now,
    timeoutMs = FETCH_TIMEOUT_MS,
    maxResponseBytes = FETCH_MAX_RESPONSE_BYTES,
    perMinute = FETCH_RATE_PER_MINUTE,
    concurrent = FETCH_MAX_CONCURRENT,
  } = {}) {
    this.lookup = lookup
    this.isBlocked = blocked
    this.isPrivate = isPrivateAddress
    this.request = request
    this.httpRequest = plainRequest
    this.ca = ca
    this.port = port
    this.now = now
    this.timeoutMs = timeoutMs
    this.maxResponseBytes = maxResponseBytes
    this.limiter = new ComponentRateLimiter({ now, perMinute, concurrent })
    /** @type {Map<string, { count: number, lastAt: string | null, hosts: Record<string, number> }>} */
    this.stats = new Map()
  }

  /** @param {string} id */
  networkStats(id) {
    const stats = this.stats.get(id)
    return stats
      ? { count: stats.count, lastAt: stats.lastAt, hosts: { ...stats.hosts } }
      : { count: 0, lastAt: null, hosts: {} }
  }

  /** 组件删除后清掉它的统计与限流状态。 @param {string} id */
  forget(id) {
    this.stats.delete(id)
    this.limiter.forget(id)
  }

  /** @param {string} id @param {string} host */
  record(id, host) {
    const stats = this.stats.get(id) || { count: 0, lastAt: null, hosts: {} }
    stats.count += 1
    stats.lastAt = new Date(this.now()).toISOString()
    if (stats.hosts[host] !== undefined || Object.keys(stats.hosts).length < MAX_TRACKED_HOSTS)
      stats.hosts[host] = (stats.hosts[host] || 0) + 1
    this.stats.set(id, stats)
  }

  /**
   * 解析主机并校验每一个地址；任何一个被阻断就整体拒绝（不挑“好的”那个连）。
   * 私网源（privateTarget）反过来：每个地址都必须落在私网段，IP 字面量不经 DNS 直接校验。
   * @param {string} hostname @param {AbortSignal} [signal] @param {boolean} [privateTarget]
   */
  async resolve(hostname, signal, privateTarget = false) {
    let addresses
    if (privateTarget && isIP(hostname)) addresses = [{ address: hostname, family: isIP(hostname) }]
    else {
      try {
        addresses = await this.lookup(hostname, signal)
      } catch {
        throw new CustomUiError('component_fetch_failed', 502, { host: hostname })
      }
    }
    if (!Array.isArray(addresses) || !addresses.length)
      throw new CustomUiError('component_fetch_failed', 502, { host: hostname })
    for (const entry of addresses) {
      const allowed =
        entry &&
        typeof entry.address === 'string' &&
        (privateTarget ? this.isPrivate(entry.address) : !this.isBlocked(entry.address))
      if (!allowed)
        throw new CustomUiError('component_fetch_address_blocked', 403, { host: hostname })
    }
    return addresses.map((entry) => ({
      address: entry.address,
      family: isIP(entry.address) === 6 ? 6 : 4,
    }))
  }

  /**
   * 发出一跳请求，只连接已经校验过的地址。
   * @param {{ url: URL, method: string, headers: Record<string, string>, body: Buffer | null }} hop
   * @param {{ address: string, family: number }[]} addresses
   * @param {AbortSignal} signal
   * @param {boolean} [privateTarget]
   * @returns {Promise<import('node:http').IncomingMessage>}
   */
  send(hop, addresses, signal, privateTarget = false) {
    return new Promise((resolveResponse, reject) => {
      const headers = {
        ...hop.headers,
        'user-agent': USER_AGENT,
        // 不要压缩：代理按字节限制体积，也不替组件解压（避免解压炸弹）。
        'accept-encoding': 'identity',
        ...(privateTarget ? { [COMPONENT_PROXY_HEADER]: '1' } : {}),
        ...(hop.body ? { 'content-length': String(hop.body.length) } : {}),
      }
      const plain = hop.url.protocol === 'http:'
      const hostname = bareHostname(hop.url)
      // 私网源按声明的端口连接；公网目标恒为 443（测试注入的本地端口）。
      const port = privateTarget ? Number(hop.url.port) || (plain ? 80 : 443) : this.port
      const request = plain ? this.httpRequest : this.request
      const req = request(
        {
          host: hostname,
          // SNI 不能是 IP 字面量；私网 IP 走 https 时只做证书校验，不发 SNI。
          ...(plain || isIP(hostname) ? {} : { servername: hostname }),
          port,
          method: hop.method,
          path: `${hop.url.pathname}${hop.url.search}`,
          headers,
          agent: false,
          signal,
          ...(this.ca && !plain ? { ca: this.ca } : {}),
          // 连接只用上面校验过的地址；Node 的自动选族会以 all:true 调用 lookup。
          lookup: (_host, options, callback) => {
            if (options && typeof options === 'object' && options.all) callback(null, addresses)
            else callback(null, addresses[0].address, addresses[0].family)
          },
        },
        resolveResponse,
      )
      req.once('error', reject)
      req.end(hop.body || undefined)
    })
  }

  /**
   * @param {import('node:http').IncomingMessage} response
   * @param {AbortSignal} signal
   * @returns {Promise<Buffer>}
   */
  readBody(response, signal) {
    const declared = Number(response.headers['content-length'])
    if (Number.isFinite(declared) && declared > this.maxResponseBytes) {
      response.destroy()
      return Promise.reject(new CustomUiError('component_fetch_too_large', 502))
    }
    return new Promise((resolveBody, reject) => {
      /** @type {Buffer[]} */
      const chunks = []
      let total = 0
      const onAbort = () => {
        response.destroy()
        reject(signal.reason)
      }
      signal.addEventListener('abort', onAbort, { once: true })
      const done = () => signal.removeEventListener('abort', onAbort)
      response.on('data', (chunk) => {
        total += chunk.length
        if (total > this.maxResponseBytes) {
          done()
          response.destroy()
          reject(new CustomUiError('component_fetch_too_large', 502))
          return
        }
        chunks.push(chunk)
      })
      response.once('end', () => {
        done()
        resolveBody(Buffer.concat(chunks))
      })
      response.once('error', (error) => {
        done()
        reject(error)
      })
    })
  }

  /**
   * Runtime 已解析认证的单跳请求：沿用 DNS 固定、响应大小和响应头白名单，绝不跟随重定向。
   * 只能由 Provider 请求服务调用；此入口不接受组件提供的认证头。
   * @param {{ url: URL, method: string, headers: Record<string, string>, body: Buffer | null }} hop
   * @param {AbortSignal} signal
   */
  async sendAuthenticated(hop, signal) {
    const url = checkFetchTarget(hop.url.toString(), [hop.url.hostname])
    signal.throwIfAborted()
    const addresses = await this.resolve(url.hostname, signal)
    signal.throwIfAborted()
    const response = await this.send({ ...hop, url }, addresses, signal)
    const status = response.statusCode || 0
    if (REDIRECT_STATUSES.has(status)) {
      response.destroy()
      throw new CustomUiError('component_provider_redirect_denied', 502)
    }
    const bytes = hop.method === 'HEAD' ? Buffer.alloc(0) : await this.readBody(response, signal)
    if (hop.method === 'HEAD') response.resume()
    const text = isTextContent(String(response.headers['content-type'] || ''))
    return {
      status,
      statusText: toWellFormed(response.statusMessage || ''),
      headers: publicResponseHeaders(response.headers),
      body: text ? toWellFormed(bytes.toString('utf8')) : bytes.toString('base64'),
      bodyEncoding: text ? 'utf8' : 'base64',
      url: url.toString(),
      redirected: false,
    }
  }

  /**
   * @param {string} componentId
   * @param {NetworkPolicy | null | undefined} network manifest 规范化后的 network 能力
   * @param {unknown} input
   */
  async fetch(componentId, network, input) {
    const declared =
      network &&
      Array.isArray(network.hosts) &&
      (network.hosts.length > 0 || (network.privateOrigins?.length ?? 0) > 0)
    if (!network || !declared)
      throw new CustomUiError('component_capability_denied', 403, { capability: 'network' })
    const parsed = parseFetchRequest(input)
    let { url, privateTarget } = resolveFetchTarget(parsed.url, network)
    const release = this.limiter.acquire(componentId)
    const controller = new AbortController()
    const timeout = new CustomUiError('component_fetch_timeout', 504)
    const timer = setTimeout(() => controller.abort(timeout), this.timeoutMs)
    timer.unref?.()
    /** @type {<T>(promise: Promise<T>) => Promise<T>} */
    const raceAbort = (promise) =>
      new Promise((resolveRace, rejectRace) => {
        if (controller.signal.aborted) return rejectRace(controller.signal.reason)
        const onAbort = () => rejectRace(controller.signal.reason)
        controller.signal.addEventListener('abort', onAbort, { once: true })
        promise.then(
          (/** @type {any} */ value) => {
            controller.signal.removeEventListener('abort', onAbort)
            resolveRace(value)
          },
          (/** @type {unknown} */ error) => {
            controller.signal.removeEventListener('abort', onAbort)
            rejectRace(error)
          },
        )
      })
    let method = parsed.method
    let body = parsed.body
    const headers = { ...parsed.headers }
    let redirected = false
    try {
      for (let hop = 0; ; hop += 1) {
        const addresses = await raceAbort(this.resolve(bareHostname(url), undefined, privateTarget))
        const response = await raceAbort(
          this.send({ url, method, headers, body }, addresses, controller.signal, privateTarget),
        )
        const status = response.statusCode || 0
        const location = response.headers.location
        if (REDIRECT_STATUSES.has(status) && typeof location === 'string' && location) {
          response.resume()
          if (hop >= FETCH_MAX_REDIRECTS)
            throw new CustomUiError('component_fetch_redirect_limit', 502)
          let nextRaw
          try {
            nextRaw = new URL(location, url).toString()
          } catch {
            throw new CustomUiError('component_fetch_invalid', 400, { field: 'location' })
          }
          const next = resolveFetchTarget(nextRaw, network, 'location')
          // 与浏览器 fetch 一致：303 一律改 GET；301/302 的 POST 改 GET。
          if (status === 303 || ((status === 301 || status === 302) && method === 'POST')) {
            method = method === 'HEAD' ? 'HEAD' : 'GET'
            body = null
            delete headers['content-type']
          }
          // 跨源重定向（主机、协议或端口变化）不带组件自己的凭据。
          if (next.url.origin !== url.origin) delete headers.authorization
          url = next.url
          privateTarget = next.privateTarget
          redirected = true
          continue
        }
        // 统计口径：拿到上游最终响应（任何状态码）的请求才计入，记最终主机（评审返修）。
        // 被地址阻断、解析失败、超时或重定向超限的请求没有连到这台主机，不能在状态栏显示成“用过的主机”。
        // url.host 对公网目标就是主机名（443 不显示）；私网源带上端口，便于区分同一台机器上的服务。
        this.record(componentId, url.host)
        const bytes =
          method === 'HEAD' ? Buffer.alloc(0) : await this.readBody(response, controller.signal)
        const contentType = String(response.headers['content-type'] || '')
        const text = isTextContent(contentType)
        return {
          status,
          statusText: toWellFormed(response.statusMessage || ''),
          headers: publicResponseHeaders(response.headers),
          body: text ? toWellFormed(bytes.toString('utf8')) : bytes.toString('base64'),
          bodyEncoding: text ? 'utf8' : 'base64',
          url: url.toString(),
          redirected,
        }
      }
    } catch (error) {
      if (error instanceof CustomUiError) throw error
      if (controller.signal.aborted) throw timeout
      throw new CustomUiError('component_fetch_failed', 502, { host: url.hostname })
    } finally {
      clearTimeout(timer)
      release()
    }
  }
}
