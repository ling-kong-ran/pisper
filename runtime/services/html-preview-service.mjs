// 会话里的本地 HTML 预览：点开回复里的 .html 路径时，右侧浏览器经这里加载页面和它的相对路径资源。
//
// 页面可能是代理写的，也可能是仓库自带的，都不可信：
// - 响应带 CSP sandbox 且不含 allow-same-origin，页面是不透明来源，读不到应用的数据；
// - iframe 的子资源请求带不上 SameSite=Strict 的桌面 Cookie，也发不了设备令牌，所以这条路由在鉴权之前，
//   靠路径里的令牌授权。令牌只有已鉴权的接口能发，只对一个目录有效，存在内存里，重启即失效；
// - 只服务这个目录下的网页资源类型；点开头的路径、看起来存着凭据的文件和指向目录外的符号链接都不给。
import { randomBytes } from 'node:crypto'
import { createReadStream } from 'node:fs'
import { realpath, stat } from 'node:fs/promises'
import { basename, dirname, extname, isAbsolute, join, relative, resolve } from 'node:path'

export const HTML_PREVIEW_PREFIX = '/_pisper/html-preview/'

const TOKEN_TTL_MS = 12 * 60 * 60 * 1000
const MAX_TOKENS = 64
const MAX_PATH_LENGTH = 4096

const CONTENT_TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.htm': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.map': 'application/json; charset=utf-8',
  '.txt': 'text/plain; charset=utf-8',
  '.csv': 'text/csv; charset=utf-8',
  '.xml': 'application/xml; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.avif': 'image/avif',
  '.bmp': 'image/bmp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.otf': 'font/otf',
  '.mp3': 'audio/mpeg',
  '.wav': 'audio/wav',
  '.ogg': 'audio/ogg',
  '.m4a': 'audio/mp4',
  '.mp4': 'video/mp4',
  '.webm': 'video/webm',
  '.wasm': 'application/wasm',
  '.glb': 'model/gltf-binary',
  '.gltf': 'model/gltf+json',
}

// JSON 这类数据文件页面常要读，但同一目录下也可能放着配置：名字像凭据的不给。
const SENSITIVE_NAME = /secret|credential|password|passwd|token|private|apikey|api[-_]key|auth/i

const RESPONSE_HEADERS = {
  'Content-Security-Policy': 'sandbox allow-scripts allow-forms allow-popups allow-modals',
  'X-Content-Type-Options': 'nosniff',
  'Cache-Control': 'no-store',
  // 页面引用 CDN 时，令牌不能跟着 Referer 发出去。
  'Referrer-Policy': 'no-referrer',
  // 不透明来源加载模块脚本、字体和 fetch 同目录文件都要过 CORS；能读到这里说明已经持有令牌。
  'Access-Control-Allow-Origin': '*',
  'Cross-Origin-Resource-Policy': 'cross-origin',
}

export class HtmlPreviewError extends Error {
  constructor(code, statusCode, message) {
    super(message)
    this.code = code
    this.statusCode = statusCode
  }
}

function isInside(root, target) {
  const path = relative(root, target)
  return Boolean(path) && !path.startsWith('..') && !isAbsolute(path)
}

function hasControlCharacters(value) {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0) || 0
    return codePoint < 32 || codePoint === 127
  })
}

function sendNotFound(res) {
  res.writeHead(404, { ...RESPONSE_HEADERS, 'Content-Type': 'text/plain; charset=utf-8' })
  res.end('Not found\n')
}

export class HtmlPreviewService {
  constructor({ now = Date.now, tokenTtlMs = TOKEN_TTL_MS, maxTokens = MAX_TOKENS } = {}) {
    this.now = now
    this.tokenTtlMs = tokenTtlMs
    this.maxTokens = maxTokens
    /** @type {Map<string, { root: string, expiresAt: number }>} */
    this.tokens = new Map()
  }

  /**
   * 为工作区内的一个 HTML 文件发令牌。每次点开都发新的：地址变了 iframe 才会重新加载改过的页面。
   * @param {{ workspace: string, path: unknown }} input
   * @returns {Promise<{ url: string }>}
   */
  async create({ workspace, path }) {
    if (
      typeof path !== 'string' ||
      !path.trim() ||
      path.length > MAX_PATH_LENGTH ||
      hasControlCharacters(path)
    )
      throw new HtmlPreviewError('html_preview_invalid_path', 400, '预览路径无效。')
    const root = await realpath(workspace).catch(() => null)
    const file = await realpath(resolve(workspace, path.trim())).catch(() => null)
    const info = file ? await stat(file).catch(() => null) : null
    if (!root || !file || !info?.isFile())
      throw new HtmlPreviewError('html_preview_not_found', 404, '要预览的文件不存在。')
    if (!isInside(root, file))
      throw new HtmlPreviewError(
        'html_preview_outside_workspace',
        403,
        '只能预览当前工作区内的文件。',
      )
    if (!['.html', '.htm'].includes(extname(file).toLowerCase()))
      throw new HtmlPreviewError('html_preview_not_html', 400, '只能预览 HTML 文件。')

    const token = randomBytes(32).toString('base64url')
    this.prune()
    this.tokens.set(token, { root: dirname(file), expiresAt: this.now() + this.tokenTtlMs })
    return { url: `${HTML_PREVIEW_PREFIX}${token}/${encodeURIComponent(basename(file))}` }
  }

  prune() {
    const now = this.now()
    for (const [token, entry] of this.tokens) if (entry.expiresAt <= now) this.tokens.delete(token)
    // Map 按插入顺序遍历，最早发的先淘汰。
    while (this.tokens.size >= this.maxTokens) this.tokens.delete(this.tokens.keys().next().value)
  }

  /** @returns {Promise<boolean>} 这条路径归预览所有时返回 true，响应已写完。 */
  async handle(req, res, url) {
    if (!url.pathname.startsWith(HTML_PREVIEW_PREFIX)) return false
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      res.writeHead(405, { ...RESPONSE_HEADERS, Allow: 'GET, HEAD' })
      res.end()
      return true
    }
    const file = await this.resolveAsset(url.pathname.slice(HTML_PREVIEW_PREFIX.length))
    if (!file) {
      sendNotFound(res)
      return true
    }
    await this.send(req, res, file)
    return true
  }

  async resolveAsset(tail) {
    const [token, ...encoded] = tail.split('/')
    const entry = this.tokens.get(token)
    if (!entry || entry.expiresAt <= this.now() || !encoded.length) return null
    let segments
    try {
      segments = encoded.map(decodeURIComponent)
    } catch {
      return null
    }
    if (
      segments.some(
        (segment) =>
          !segment ||
          segment.startsWith('.') ||
          /[\\/]/.test(segment) ||
          hasControlCharacters(segment),
      )
    )
      return null
    const name = segments.at(-1)
    const contentType = CONTENT_TYPES[extname(name).toLowerCase()]
    if (!contentType || SENSITIVE_NAME.test(name)) return null
    const path = await realpath(join(entry.root, ...segments)).catch(() => null)
    if (!path || !isInside(entry.root, path)) return null
    const info = await stat(path).catch(() => null)
    return info?.isFile() ? { path, size: info.size, contentType } : null
  }

  async send(req, res, { path, size, contentType }) {
    const headers = { ...RESPONSE_HEADERS, 'Content-Type': contentType, 'Content-Length': size }
    if (req.method === 'HEAD') {
      res.writeHead(200, headers)
      res.end()
      return
    }
    await new Promise((done) => {
      const stream = createReadStream(path)
      stream.once('open', () => {
        res.writeHead(200, headers)
        stream.pipe(res)
      })
      stream.once('error', () => {
        if (!res.headersSent) sendNotFound(res)
        else res.destroy()
        done()
      })
      stream.once('end', done)
      res.once('close', () => {
        stream.destroy()
        done()
      })
    })
  }
}
