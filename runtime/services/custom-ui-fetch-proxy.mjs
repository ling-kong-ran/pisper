// 组件网络代理：原生组件的 ctx.net.fetch 一律经 POST /api/custom-ui/components/:id/fetch 走这里。
//
// 规则（规格 5.8、计划 §3.7）：
// - 只放行 manifest 声明过的主机，只允许 https、443 端口，不允许 userinfo 与 IP 字面量；
// - 先自己解析 DNS、校验全部地址，再只把校验过的 IP 交给连接（防 DNS rebinding）；
// - 手动跟随重定向（≤5 跳），每一跳重新做全部校验，跨主机时去掉组件自带的 Authorization；
// - 10 秒总超时，响应 >2 MiB 立即中断；请求头与响应头都按白名单；
// - 绝不转发应用请求里的 Cookie / Authorization / Origin：代理从空白开始构造请求；
// - 每个组件令牌桶每分钟 30 次、并发 ≤4；统计只记主机、方法、状态码、耗时，不记查询参数与请求体。
import { lookup as dnsLookup } from 'node:dns/promises'
import { request as httpsRequest } from 'node:https'
import { isIP } from 'node:net'
import { toWellFormed } from '../../shared/custom-ui-manifest.mjs'
import { isBlockedAddress, isBlockedHostname } from '../security/public-address.mjs'
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
   *   lookup?: (hostname: string) => Promise<{ address: string, family: number }[]>,
   *   isBlockedAddress?: (address: string) => boolean,
   *   request?: typeof httpsRequest,
   *   ca?: string | Buffer,
   *   port?: number,
   *   now?: () => number,
   *   timeoutMs?: number,
   *   maxResponseBytes?: number,
   *   perMinute?: number,
   *   concurrent?: number,
   * }} [options]
   *   lookup / isBlockedAddress / ca / port 只供测试注入（本地 TLS 服务）；产品路径一律用默认值。
   */
  constructor({
    lookup = (hostname) => dnsLookup(hostname, { all: true, verbatim: true }),
    isBlockedAddress: blocked = (address) => isBlockedAddress(address),
    request = httpsRequest,
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
    this.request = request
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
   * @param {string} hostname
   */
  async resolve(hostname) {
    let addresses
    try {
      addresses = await this.lookup(hostname)
    } catch {
      throw new CustomUiError('component_fetch_failed', 502, { host: hostname })
    }
    if (!Array.isArray(addresses) || !addresses.length)
      throw new CustomUiError('component_fetch_failed', 502, { host: hostname })
    for (const entry of addresses) {
      if (!entry || typeof entry.address !== 'string' || this.isBlocked(entry.address))
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
   * @returns {Promise<import('node:http').IncomingMessage>}
   */
  send(hop, addresses, signal) {
    return new Promise((resolveResponse, reject) => {
      const headers = {
        ...hop.headers,
        'user-agent': USER_AGENT,
        // 不要压缩：代理按字节限制体积，也不替组件解压（避免解压炸弹）。
        'accept-encoding': 'identity',
        ...(hop.body ? { 'content-length': String(hop.body.length) } : {}),
      }
      const req = this.request(
        {
          host: hop.url.hostname,
          servername: hop.url.hostname,
          port: this.port,
          method: hop.method,
          path: `${hop.url.pathname}${hop.url.search}`,
          headers,
          agent: false,
          signal,
          ...(this.ca ? { ca: this.ca } : {}),
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
   * @param {string} componentId
   * @param {{ hosts: readonly string[] } | null | undefined} network manifest 规范化后的 network 能力
   * @param {unknown} input
   */
  async fetch(componentId, network, input) {
    if (!network || !Array.isArray(network.hosts) || !network.hosts.length)
      throw new CustomUiError('component_capability_denied', 403, { capability: 'network' })
    const parsed = parseFetchRequest(input)
    const hosts = network.hosts
    let url = checkFetchTarget(parsed.url, hosts)
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
    this.record(componentId, url.hostname)
    try {
      for (let hop = 0; ; hop += 1) {
        const addresses = await raceAbort(this.resolve(url.hostname))
        const response = await raceAbort(
          this.send({ url, method, headers, body }, addresses, controller.signal),
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
          const next = checkFetchTarget(nextRaw, hosts, 'location')
          // 与浏览器 fetch 一致：303 一律改 GET；301/302 的 POST 改 GET。
          if (status === 303 || ((status === 301 || status === 302) && method === 'POST')) {
            method = method === 'HEAD' ? 'HEAD' : 'GET'
            body = null
            delete headers['content-type']
          }
          // 跨主机重定向不带组件自己的凭据。
          if (next.hostname !== url.hostname) delete headers.authorization
          url = next
          redirected = true
          continue
        }
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
