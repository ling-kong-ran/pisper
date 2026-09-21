// 自定义 UI 组件服务：扫描 dataDir/custom-ui/<component-id>/ 下用户自写的
// 静态组件（manifest.json + HTML/JS/CSS），向前端提供组件清单、沙箱 iframe
// 资产与 postMessage 桥接脚本。
//
// 安全模型：
// - 组件资产以 iframe sandbox="allow-scripts"（不含 allow-same-origin）加载，
//   处于 opaque origin，无法直接访问主站 API/DOM/存储；
//   与应用的交互只能走父页面代理的 postMessage 桥，能力按 manifest.permissions 过滤。
// - 资产读取限制在组件目录内：拒绝绝对路径、.. 穿越与符号链接逃逸；
//   manifest.json 与隐藏文件永不作为资产返回。
// - 组件目录完全由用户在本机放置，runtime 不做远程安装/下载。
import { randomBytes } from 'node:crypto'
import { parse, serialize } from 'parse5'
import { createReadStream } from 'node:fs'
import { readFile, readdir, realpath, stat } from 'node:fs/promises'
import { homedir } from 'node:os'
import { extname, isAbsolute, join, normalize, resolve, sep } from 'node:path'
import { readJson } from '../storage/json-file.mjs'

// 组件 id 即目录名：只允许安全的文件名字符，避免路径与 URL 编码问题。
const COMPONENT_ID_PATTERN = /^[a-z0-9][a-z0-9._-]{0,63}$/

// manifest 声明的桥接能力白名单；父页面按声明过滤 postMessage 方法。
export const CUSTOM_UI_PERMISSIONS = Object.freeze([
  // 只读数据
  'config.read',
  'sessions.read',
  // 交互反馈
  'notify',
])

const ASSET_MIME = {
  '.html': 'text/html; charset=utf-8',
  '.htm': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
}

// 资产大小上限：组件应是轻量静态页面，防止把目录当成文件服务器滥用。
const MAX_ASSET_BYTES = 8 * 1024 * 1024
const MAX_MANIFEST_BYTES = 64 * 1024
const VIEW_TTL_MS = 5 * 60_000
const MAX_VIEWS = 128

function asRecord(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : null
}

// 展示路径缩写：不把用户完整 home 目录路径泄漏到界面/日志。
function displayPath(path) {
  const home = homedir()
  if (home && (path === home || path.startsWith(`${home}${sep}`))) {
    return `~${path.slice(home.length)}`
  }
  return path
}

// 归一化 manifest；非法字段抛错（带组件 id，便于用户定位自己的配置问题）。
export function normalizeComponentManifest(id, raw) {
  const manifest = asRecord(raw)
  if (!manifest) throw new Error(`组件 ${id} 的 manifest.json 必须是 JSON 对象。`)
  const name = String(manifest.name || '').trim()
  if (!name || name.length > 120) throw new Error(`组件 ${id} 的 name 缺失或过长。`)
  const entry = String(manifest.entry || 'index.html').trim() || 'index.html'
  if (isAbsolute(entry) || entry.includes('..') || entry.startsWith('/'))
    throw new Error(`组件 ${id} 的 entry 必须是目录内的相对路径。`)
  const version = String(manifest.version || '')
    .trim()
    .slice(0, 64)
  const description = String(manifest.description || '')
    .trim()
    .slice(0, 500)
  const rawPermissions = Array.isArray(manifest.permissions) ? manifest.permissions : []
  const permissions = [
    ...new Set(
      rawPermissions
        .map((item) => String(item || '').trim())
        .filter((item) => CUSTOM_UI_PERMISSIONS.includes(item)),
    ),
  ]
  return { id, name, version, description, entry, permissions }
}

// postMessage 桥接脚本：组件 HTML 通过 <script src="/api/custom-ui/bridge.js"> 引入，
// 获得 window.pisper 调用面；所有请求经父页面代理并携带请求 id 配对响应。
// 注意：脚本运行在 opaque origin，不能 fetch 主站 API，也不能读父页面任何状态。
const BRIDGE_SCRIPT = String.raw`// Pisper 自定义 UI 桥：与父页面（应用壳）通过 postMessage 通信。
(function () {
  if (window.pisper) return
  var nextId = 1
  var pending = new Map()
  var themeListeners = new Set()
  function post(method, params) {
    return new Promise(function (resolve, reject) {
      var id = nextId++
      pending.set(id, { resolve: resolve, reject: reject })
      parent.postMessage({ pisperBridge: 1, id: id, method: method, params: params || {} }, '*')
    })
  }
  window.addEventListener('message', function (event) {
    var data = event.data
    if (event.source !== parent || !data || data.pisperBridge !== 1) return
    if (data.type === 'theme') {
      applyTheme(data.theme || {})
      themeListeners.forEach(function (listener) {
        try { listener(data.theme || {}) } catch (error) { console.error(error) }
      })
      return
    }
    var slot = pending.get(data.id)
    if (!slot) return
    pending.delete(data.id)
    if (data.ok) {
      // 握手响应携带主题：就绪即应用，保证首帧渲染不闪烁。
      if (data.result && data.result.theme) applyTheme(data.result.theme)
      slot.resolve(data.result)
    } else {
      slot.reject(new Error(String(data.error || 'Pisper bridge call failed')))
    }
  })
  function applyTheme(theme) {
    var root = document.documentElement
    root.dataset.pisperTheme = theme.mode === 'dark' ? 'dark' : 'light'
    root.style.colorScheme = root.dataset.pisperTheme
    var vars = theme.variables || {}
    Object.keys(vars).forEach(function (name) {
      if (/^--[a-z0-9-]+$/i.test(name)) root.style.setProperty(name, String(vars[name]))
    })
  }
  window.pisper = {
    // 握手：返回组件信息、声明的能力与当前主题。
    ready: function () { return post('ready') },
    getConfig: function () { return post('getConfig') },
    listSessions: function (params) { return post('listSessions', params) },
    notify: function (message, kind) { return post('notify', { message: message, kind: kind }) },
    onThemeChanged: function (listener) {
      themeListeners.add(listener)
      return function () { themeListeners.delete(listener) }
    },
  }
})()
`

export class CustomUiService {
  constructor({ dataDir, now = Date.now }) {
    this.root = join(dataDir, 'custom-ui')
    this.now = now
    this.views = new Map()
  }

  // 仅由已鉴权的父页面签发；凭证只能读取单个组件静态资源，不能用于任何应用 API。
  async createView(id, owner = 'local', origin = 'http://localhost') {
    let source
    try {
      source = new URL(origin)
    } catch {
      throw Object.assign(new Error('组件来源无效。'), { statusCode: 400 })
    }
    if (!['http:', 'https:'].includes(source.protocol) || source.origin !== origin) {
      throw Object.assign(new Error('组件来源无效。'), { statusCode: 400 })
    }
    if (!COMPONENT_ID_PATTERN.test(id))
      throw Object.assign(new Error('组件不存在。'), { statusCode: 404 })
    const manifest = await this.readManifest(id)
    if (!manifest || !(await this.resolveAssetPath(id, manifest.entry))) {
      throw Object.assign(new Error('组件资源不存在。'), { statusCode: 404 })
    }
    for (const key of this.views.keys()) this.getView(key)
    if (this.views.size >= MAX_VIEWS)
      throw Object.assign(new Error('组件预览数量已达上限。'), { statusCode: 429 })
    const viewId = randomBytes(32).toString('hex')
    this.views.set(viewId, { componentId: id, owner, origin, expiresAt: this.now() + VIEW_TTL_MS })
    return {
      id: viewId,
      entryUrl: `/api/custom-ui/render/${viewId}/assets/${manifest.entry.split('/').map(encodeURIComponent).join('/')}`,
    }
  }

  getView(id) {
    const view = this.views.get(id)
    if (view && view.expiresAt > this.now()) return view
    this.views.delete(id)
    return null
  }

  renewView(id, owner = 'local') {
    const view = this.getView(id)
    if (!view || view.owner !== owner)
      throw Object.assign(new Error('组件预览已过期。'), { statusCode: 404 })
    view.expiresAt = this.now() + VIEW_TTL_MS
  }

  revokeView(id, owner = 'local') {
    if (this.views.get(id)?.owner === owner) this.views.delete(id)
  }

  dispose() {
    this.views.clear()
  }

  // 组件目录绝对路径；目录不存在视为无组件。
  componentDir(id) {
    return join(this.root, id)
  }

  async readManifest(id) {
    const manifestPath = join(this.componentDir(id), 'manifest.json')
    const info = await stat(manifestPath).catch(() => null)
    if (!info?.isFile() || info.size > MAX_MANIFEST_BYTES) return null
    const raw = await readJson(manifestPath, null).catch(() => null)
    if (raw === null) return null
    try {
      return normalizeComponentManifest(id, raw)
    } catch {
      // manifest 非法的组件不进入列表，错误详情由 describeComponent 暴露。
      return null
    }
  }

  // 列出全部可用组件（含目录路径，方便用户放置/编辑文件）。
  async listComponents() {
    const entries = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    const components = []
    for (const entry of entries) {
      if (!entry.isDirectory() || !COMPONENT_ID_PATTERN.test(entry.name)) continue
      const manifest = await this.readManifest(entry.name)
      if (!manifest) continue
      components.push({
        ...manifest,
        entryUrl: `/api/custom-ui/components/${encodeURIComponent(manifest.id)}/assets/${manifest.entry
          .split('/')
          .map(encodeURIComponent)
          .join('/')}`,
        directory: displayPath(this.componentDir(manifest.id)),
      })
    }
    components.sort((left, right) => left.name.localeCompare(right.name))
    return { root: displayPath(this.root), components }
  }

  // 资产解析：返回组件目录内的绝对路径；任何越界尝试返回 null。
  async resolveAssetPath(id, relativePath) {
    if (!COMPONENT_ID_PATTERN.test(id)) return null
    const rawPath = String(relativePath || '')
    if (isAbsolute(rawPath) || rawPath.split(/[/\\]/).some((part) => part === '..')) return null
    const relative = normalize(rawPath)
    if (!relative || relative.startsWith('..') || isAbsolute(relative)) return null
    const segments = relative.split(sep)
    if (segments.some((segment) => segment.startsWith('.'))) return null
    // manifest 属于配置元数据，不作为静态资产提供。
    if (segments.length === 1 && segments[0] === 'manifest.json') return null
    const dir = resolve(this.componentDir(id))
    const file = resolve(dir, relative)
    if (file !== dir && !file.startsWith(`${dir}${sep}`)) return null
    // 符号链接防护：realpath 后仍须落在组件目录内。
    const info = await stat(file).catch(() => null)
    if (!info?.isFile() || info.size > MAX_ASSET_BYTES) return null
    const [realFile, realDir, realRoot] = await Promise.all([
      realpath(file).catch(() => ''),
      realpath(dir).catch(() => ''),
      realpath(this.root).catch(() => ''),
    ])
    if (!realRoot || !realDir.startsWith(`${realRoot}${sep}`)) return null
    if (!realFile || (realFile !== realDir && !realFile.startsWith(`${realDir}${sep}`))) return null
    return { file, size: info.size }
  }

  // 直接写出资产响应（流式 + 内容类型 + 禁止缓存：用户本地迭代组件时立即生效）。
  async serveAsset({ id, path, res, json, resourceBase }) {
    const target = await this.resolveAssetPath(id, path)
    if (!target) {
      json(404, { error: '组件资源不存在。' })
      return
    }
    const mime = ASSET_MIME[extname(target.file).toLowerCase()] || 'application/octet-stream'
    const headers = this.assetHeaders(resourceBase)
    // 保留旧的受鉴权资产 URL，但同样强制 CSP 沙箱；只有凭证入口改写桥脚本地址。
    if (resourceBase && mime.startsWith('text/html')) {
      const document = parse(await readFile(target.file, 'utf8'))
      const rewriteBridge = (node) => {
        if (node.tagName === 'script') {
          const src = node.attrs?.find((attribute) => attribute.name === 'src')
          if (src?.value === '/api/custom-ui/bridge.js') src.value = `${resourceBase}bridge.js`
        }
        for (const child of node.childNodes || []) rewriteBridge(child)
      }
      rewriteBridge(document)
      const html = serialize(document)
      res.writeHead(200, {
        ...headers,
        'Content-Type': mime,
        'Content-Length': Buffer.byteLength(html),
      })
      res.end(html)
      return
    }
    res.writeHead(200, { ...headers, 'Content-Type': mime, 'Content-Length': target.size })
    createReadStream(target.file)
      .on('error', () => {
        if (!res.headersSent) json(404, { error: '组件资源不存在。' })
        else res.destroy()
      })
      .pipe(res)
  }

  assetHeaders(resourceBase) {
    const source = resourceBase || "'none'"
    return {
      'Cache-Control': 'no-store',
      'X-Content-Type-Options': 'nosniff',
      'Referrer-Policy': 'no-referrer',
      'Access-Control-Allow-Origin': '*',
      // sandbox 必须出现在 HTTP 响应中，直接打开 HTML/SVG 也不能成为主站同源文档。
      'Content-Security-Policy': [
        'sandbox allow-scripts',
        "default-src 'none'",
        `script-src 'unsafe-inline' ${source}`,
        `style-src 'unsafe-inline' ${source}`,
        `img-src data: blob: ${source}`,
        `font-src data: ${source}`,
        `connect-src ${source}`,
        "base-uri 'none'",
        "form-action 'none'",
        "frame-ancestors 'self'",
      ].join('; '),
    }
  }

  bridgeScript() {
    return BRIDGE_SCRIPT
  }
}
