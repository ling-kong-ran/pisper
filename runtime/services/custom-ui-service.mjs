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
// - 用户可本机放置目录，或经鉴权 API 导入 ZIP；Runtime 不做远程下载。
//
// A7 起同一个服务也承载原生组件（v2）：目录分类、内容哈希、按 rev 服务 ESM 模块、启用状态、
// 放置实例与模板创建；旧 iframe 机制在 S4 删除前保持原样，GET /api/custom-ui/components 仍是旧输出。
import { randomBytes } from 'node:crypto'
import { parse, serialize } from 'parse5'
import { createReadStream } from 'node:fs'
import { lstat, readFile, readdir, rm, stat } from 'node:fs/promises'
import { extname, isAbsolute, join, normalize, sep } from 'node:path'
import {
  RESERVED_COMPONENT_IDS,
  classifyManifest,
  isSafeComponentPath,
  manifestPlacement,
  normalizeNativeManifest,
  toWellFormed,
  validateComponentProps,
} from '../../shared/custom-ui-manifest.mjs'
import { readJson } from '../storage/json-file.mjs'
import { displayCustomUiPath } from './custom-ui-path.mjs'
import { BUILTIN_CUSTOM_UI_COMPONENTS } from './custom-ui-builtins.mjs'
import { importCustomUiZip, CUSTOM_UI_MANIFEST_MAX_BYTES } from './custom-ui-import.mjs'
import { CustomUiError } from './custom-ui-error.mjs'
import {
  ComponentHasher,
  componentFileType,
  listComponentFiles,
  moveComponentToTrash,
  resolveComponentFile,
  snapshotComponent,
  writeComponentDirectory,
} from './custom-ui-files.mjs'
import {
  CustomUiInstanceStore,
  isComponentSurface,
  parseInstanceScope,
  parseInstanceTitle,
} from './custom-ui-instances.mjs'
import { CustomUiStateStore } from './custom-ui-state.mjs'
import { ComponentErrorLog } from './custom-ui-errors.mjs'
import { ComponentFetchProxy } from './custom-ui-fetch-proxy.mjs'
import { CustomUiStorage } from './custom-ui-storage.mjs'
import { ComponentWatcher } from './custom-ui-watcher.mjs'
import {
  COMPONENT_TEMPLATES,
  buildComponentTemplate,
  templateDefaultId,
} from './custom-ui-templates.mjs'

// 组件 id 即目录名：只允许安全的文件名字符，避免路径与 URL 编码问题。
const COMPONENT_ID_PATTERN = /^[a-z0-9][a-z0-9._-]{0,63}$/

// manifest 声明的桥接能力白名单；父页面按声明过滤 postMessage 方法。
export const CUSTOM_UI_PERMISSIONS = Object.freeze([
  // 只读数据
  'config.read',
  'sessions.read',
  'game-assets.read',
  // 工作流写入与运行独立授权；只读组件不能上传素材或启动任务。
  'game-assets.write',
  'game-assets.run',
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
const MAX_MANIFEST_BYTES = CUSTOM_UI_MANIFEST_MAX_BYTES
const VIEW_TTL_MS = 5 * 60_000
const MAX_VIEWS = 128

function asRecord(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : null
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
    gameAssets: {
      list: function (params) { return post('gameAssets.list', params) },
      save: function (params) { return post('gameAssets.save', params) },
      run: function (params) { return post('gameAssets.run', params) },
      stop: function (params) { return post('gameAssets.stop', params) },
      uploadImage: function (params) { return post('gameAssets.uploadImage', params) },
      image: function (params) { return post('gameAssets.image', params) },
      process: function (params) { return post('gameAssets.process', params) },
      engine: function (params) { return post('gameAssets.engine', params) },
      export: function (params) { return post('gameAssets.export', params) },
      editFrames: function (params) { return post('gameAssets.editFrames', params) },
      remove: function (params) { return post('gameAssets.remove', params) },
    },
    notify: function (message, kind) { return post('notify', { message: message, kind: kind }) },
    onThemeChanged: function (listener) {
      themeListeners.add(listener)
      return function () { themeListeners.delete(listener) }
    },
  }
})()
`

export class CustomUiService {
  constructor({
    dataDir,
    now = Date.now,
    builtinComponents = BUILTIN_CUSTOM_UI_COMPONENTS,
    randomId,
    renameDirectory,
    fetchProxy,
    errorLog,
    createWatcher,
  }) {
    this.root = join(dataDir, 'custom-ui')
    this.now = now
    this.views = new Map()
    this.builtins = new Map(builtinComponents.map((component) => [component.id, component]))
    // 原生组件（v2）的存储：版本快照、回收站与组件存储都在被监听、被服务的 custom-ui/ 之外。
    this.versionsRoot = join(dataDir, 'custom-ui-versions')
    this.trashRoot = join(dataDir, 'custom-ui-trash')
    this.dataRoot = join(dataDir, 'custom-ui-data')
    this.state = new CustomUiStateStore({ path: join(dataDir, 'custom-ui-state.json'), now })
    this.instanceStore = new CustomUiInstanceStore({
      path: join(dataDir, 'custom-ui-instances.json'),
      now,
      ...(randomId ? { randomId } : {}),
    })
    this.hasher = new ComponentHasher()
    this.renameDirectory = renameDirectory
    /** @type {Map<string, Promise<unknown>>} */
    this.locks = new Map()
    this.hostVersion = null
    this.onChange = null
    // S2：网络代理、组件存储、错误环形缓冲与文件监听。代理与错误日志只在内存里保存统计。
    this.fetchProxy = fetchProxy || new ComponentFetchProxy({ now })
    this.errorLog = errorLog || new ComponentErrorLog({ now })
    this.storage = new CustomUiStorage({ root: this.dataRoot, now })
    /** @type {(options: { root: string, onChange: (id: string) => void }) => { start: () => Promise<unknown>, dispose: () => void }} */
    this.createWatcher = createWatcher || ((options) => new ComponentWatcher(options))
    /** @type {{ start: () => Promise<unknown>, dispose: () => void } | null} */
    this.watcher = null
    // 每个组件最近一次通知的内容键（rev，或非 ready 时的状态与错误）：监听器据此去重，
    // 工具写入后立即发的事件与随后监听器的事件不会重复。
    /** @type {Map<string, string>} */
    this.notifiedKeys = new Map()
    this.instances = {
      list: (surface) => this.listInstances(surface),
      create: (input) => this.createInstance(input),
      update: (instanceId, patch) => this.updateInstance(instanceId, patch),
      remove: (instanceId) => this.removeInstance(instanceId),
      reorder: (input) => this.reorderInstances(input),
    }
  }

  async importBundle(bytes) {
    return importCustomUiZip({
      root: this.root,
      bytes,
      normalizeManifest: normalizeComponentManifest,
      reservedIds: [...this.builtins.keys()],
    })
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
    this.onChange = null
    this.watcher?.dispose()
    this.watcher = null
  }

  // 组件目录绝对路径；目录不存在视为无组件。
  componentDir(id) {
    return join(this.root, id)
  }

  async readManifest(id) {
    const builtin = this.builtins.get(id)
    if (builtin) return normalizeComponentManifest(id, builtin.manifest)
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
    const components = [...this.builtins.values()].map((component) => ({
      ...normalizeComponentManifest(component.id, component.manifest),
      entryUrl: `/api/custom-ui/components/${encodeURIComponent(component.id)}/assets/${component.manifest.entry}`,
      directory: '',
      builtIn: true,
    }))
    for (const entry of entries) {
      if (
        !entry.isDirectory() ||
        !COMPONENT_ID_PATTERN.test(entry.name) ||
        this.builtins.has(entry.name)
      )
        continue
      const manifest = await this.readManifest(entry.name)
      if (!manifest) continue
      components.push({
        ...manifest,
        entryUrl: `/api/custom-ui/components/${encodeURIComponent(manifest.id)}/assets/${manifest.entry
          .split('/')
          .map(encodeURIComponent)
          .join('/')}`,
        directory: displayCustomUiPath(this.componentDir(manifest.id)),
      })
    }
    components.sort((left, right) => left.name.localeCompare(right.name))
    return { root: displayCustomUiPath(this.root), components }
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
    const builtin = this.builtins.get(id)
    if (builtin) {
      // 内置 ID 为保留项，缺失资源也不回退到同名用户目录。
      const assetPath = segments.join('/')
      if (!Object.hasOwn(builtin.assets, assetPath)) return null
      const content = builtin.assets[assetPath]
      return { assetPath, content, size: Buffer.byteLength(content) }
    }
    // 用户目录的越界、隐藏段与符号链接防护已迁到 custom-ui-files.mjs，原生模块路由共用同一份。
    return resolveComponentFile(this.root, id, rawPath, { maxBytes: MAX_ASSET_BYTES })
  }

  // 直接写出资产响应（流式 + 内容类型 + 禁止缓存：用户本地迭代组件时立即生效）。
  async serveAsset({ id, path, res, json, resourceBase }) {
    const target = await this.resolveAssetPath(id, path)
    if (!target) {
      json(404, { error: '组件资源不存在。' })
      return
    }
    const mime =
      ASSET_MIME[extname(target.assetPath ?? target.file).toLowerCase()] ||
      'application/octet-stream'
    const headers = this.assetHeaders(resourceBase)
    // 保留旧的受鉴权资产 URL，但同样强制 CSP 沙箱；只有凭证入口改写桥脚本地址。
    if (resourceBase && mime.startsWith('text/html')) {
      const document = parse(target.content ?? (await readFile(target.file, 'utf8')))
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
    if (target.content !== undefined) {
      res.end(target.content)
      return
    }
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

  // ── 原生组件（v2）────────────────────────────────────────────────────

  /**
   * 由 app-runtime 在 base ready 之前的装配阶段调用：登记变化通知（看板事件流）与应用版本（minHostVersion），
   * 并在后台启动文件监听、清理残留的导入暂存目录。导入本模块不会启动任何长期副作用。
   * 返回的 Promise 永不拒绝（监听失败会退回轮询，清理失败只是留下残留目录），调用方可以不等。
   * 监听要显式 watch:true（app-runtime 打开）：测试与一次性调用方不需要后台监听。
   * @param {{ onChange?: (id: string, detail: { rev?: string | null, reason: string }) => void, hostVersion?: string | null, watch?: boolean }} [options]
   */
  start({ onChange, hostVersion = null, watch = false } = {}) {
    this.onChange = typeof onChange === 'function' ? onChange : null
    this.hostVersion = typeof hostVersion === 'string' && hostVersion ? hostVersion : null
    const tasks = [this.cleanupImportStaging()]
    if (watch && !this.watcher) {
      this.watcher = this.createWatcher({
        root: this.root,
        onChange: (id) => {
          void this.handleWatchedChange(id).catch(() => {})
        },
      })
      tasks.push(this.watcher.start())
    }
    return Promise.allSettled(tasks).then(() => undefined)
  }

  /**
   * 旧 ZIP 导入在 custom-ui/.component-import-* 里解包，进程中途退出会留下残留目录。
   * 只删超过 10 分钟的（同一数据目录可能有另一个进程正在导入）；.component-old-* 不碰：
   * 写入器交换中断时它可能是组件唯一的副本。
   */
  async cleanupImportStaging() {
    const entries = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    const cutoff = this.now() - 10 * 60_000
    for (const entry of entries) {
      if (!entry.isDirectory() || !entry.name.startsWith('.component-import-')) continue
      const path = join(this.root, entry.name)
      const info = await lstat(path).catch(() => null)
      if (!info?.isDirectory() || info.mtimeMs > cutoff) continue
      await rm(path, { recursive: true, force: true }).catch(() => {})
    }
  }

  /**
   * 监听器回调：重新算 rev，与上次通知的内容键相同就不发（写入器自己已经发过）。
   * @param {string} id
   */
  async handleWatchedChange(id) {
    this.hasher.invalidate(this.componentDir(id))
    const inspected = await this.inspect(id)
    if (!inspected) {
      // 目录没了（手动删除）；DELETE 接口自己已经发过 deleted，这里不重复。
      if (this.notifiedKeys.get(id) === 'deleted') return
      this.notify(id, { rev: null, reason: 'deleted' })
      return
    }
    const key = changeKey(inspected.row)
    if (this.notifiedKeys.get(id) === key) return
    this.notify(id, { rev: inspected.row.rev ?? null, reason: 'files' }, key)
  }

  /** 代理工具只拿窄接口：写入组件与描述组件，不接触实例、存储或旧 iframe 机制。 */
  agentInterface() {
    return {
      writeComponent: (input, options) => this.writeComponent(input, options),
      describe: (id) => this.describe(id),
    }
  }

  /**
   * @param {string} id
   * @param {{ rev?: string | null, reason: string }} detail
   * @param {string} [key] 去重用的内容键；不给时按 rev 推断（实例变化不改内容键）
   */
  notify(id, detail, key) {
    if (key) this.notifiedKeys.set(id, key)
    else if (detail.reason === 'deleted') this.notifiedKeys.set(id, 'deleted')
    else if (detail.rev) this.notifiedKeys.set(id, detail.rev)
    try {
      this.onChange?.(id, detail)
    } catch {
      // 事件流只是通知：通知失败不能让已经落盘的写入报错。
    }
  }

  // 每个组件一条串行链：写入、快照、删除不交错。
  withComponentLock(id, task) {
    const previous = this.locks.get(id) || Promise.resolve()
    const next = previous.catch(() => {}).then(task)
    const settled = next.catch(() => {})
    this.locks.set(id, settled)
    settled.then(() => {
      if (this.locks.get(id) === settled) this.locks.delete(id)
    })
    return next
  }

  /**
   * 读取并分类一个组件目录：native（ready / invalid）、legacy（unsupported）、broken（invalid）。
   * 目录不存在返回 null。
   */
  async inspect(id) {
    if (!COMPONENT_ID_PATTERN.test(id)) return null
    const dir = this.componentDir(id)
    const dirInfo = await lstat(dir).catch(() => null)
    if (!dirInfo?.isDirectory()) return null
    const state = await this.state.get(id)
    const manifestPath = join(dir, 'manifest.json')
    const manifestInfo = await lstat(manifestPath).catch(() => null)
    const base = {
      id,
      directory: displayCustomUiPath(dir),
      enabled: false,
      source: state?.source || 'local',
      entry: '',
      errors: [],
      updatedAt: new Date(Math.max(dirInfo.mtimeMs, manifestInfo?.mtimeMs || 0)).toISOString(),
    }
    const broken = (code) => ({
      row: {
        ...base,
        kind: 'broken',
        status: 'invalid',
        name: id,
        version: '',
        description: '',
        errors: [{ path: 'manifest.json', code }],
      },
      manifest: null,
      raw: null,
    })
    if (!manifestInfo?.isFile()) return broken('manifest_missing')
    if (manifestInfo.size > MAX_MANIFEST_BYTES) return broken('manifest_too_large')
    let raw
    try {
      raw = JSON.parse(await readFile(manifestPath, 'utf8'))
    } catch {
      return broken('manifest_json_invalid')
    }
    const kind = classifyManifest(raw)
    if (kind === 'broken') return broken('manifest_not_object')
    const text = (value, max) =>
      typeof value === 'string' ? toWellFormed(value.trim()).slice(0, max) : ''
    if (kind === 'legacy') {
      return {
        row: {
          ...base,
          kind: 'legacy',
          status: 'unsupported',
          name: text(raw.name, 120) || id,
          version: text(raw.version, 64),
          description: text(raw.description, 500),
        },
        manifest: null,
        raw,
      }
    }
    const normalized = normalizeNativeManifest(id, raw, { hostVersion: this.hostVersion })
    const nativeBase = {
      ...base,
      kind: 'native',
      enabled: state ? state.enabled : true,
      name: normalized.manifest?.name || text(raw.name, 120) || id,
      version: normalized.manifest?.version || text(raw.version, 64),
      description: normalized.manifest?.description || text(raw.description, 500),
      entry: normalized.manifest?.entry || '',
    }
    if (!normalized.manifest) {
      return {
        row: { ...nativeBase, status: 'invalid', errors: normalized.errors },
        manifest: null,
        raw,
      }
    }
    const manifest = normalized.manifest
    const hashed = await this.hasher.compute(dir)
    const newest = hashed.files.reduce(
      (latest, entry) => Math.max(latest, entry.mtimeMs),
      Date.parse(base.updatedAt),
    )
    const errors = []
    if (hashed.error) errors.push({ path: '', code: hashed.error })
    else {
      const paths = new Set(hashed.files.map((entry) => entry.path))
      if (!paths.has(manifest.entry)) errors.push({ path: 'entry', code: 'entry_missing' })
      manifest.styles.forEach((style, index) => {
        if (!paths.has(style)) errors.push({ path: `styles.${index}`, code: 'style_missing' })
      })
    }
    const row = {
      ...nativeBase,
      status: errors.length ? 'invalid' : 'ready',
      ...(errors.length ? {} : { rev: hashed.rev }),
      manifest,
      errors,
      updatedAt: new Date(newest).toISOString(),
    }
    return { row, manifest: errors.length ? null : manifest, raw }
  }

  /** v2 组件列表（S4 起由 GET /api/custom-ui/components 返回；这一阶段只供新接口与测试使用）。 */
  async catalog() {
    const entries = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    const components = []
    for (const entry of entries) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue
      if (!COMPONENT_ID_PATTERN.test(entry.name)) continue
      const inspected = await this.inspect(entry.name)
      if (inspected) components.push(inspected.row)
    }
    components.sort(
      (left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id),
    )
    return {
      root: displayCustomUiPath(this.root),
      // 5a 没有导入、版本与导出；删除是移入回收站。前端据此置灰按钮，不新增值为 false 的 Runtime 能力。
      features: { import: false, versions: false, export: false, trash: 'move' },
      components,
    }
  }

  async requireComponent(id) {
    const inspected = await this.inspect(id)
    if (!inspected) throw new CustomUiError('component_not_found', 404)
    return inspected
  }

  async describe(id) {
    const inspected = await this.requireComponent(id)
    const listed = await listComponentFiles(this.componentDir(id))
    return {
      component: inspected.row,
      files: listed.files.map((entry) => ({
        path: entry.path,
        size: entry.size,
        type: componentFileType(entry.path) || 'application/octet-stream',
      })),
      errors: inspected.row.errors,
      // 静态扫描在 A8 的导入信任确认里做；这里如实标注 unsupported，不返回空结果冒充“已扫描”。
      scan: { status: 'unsupported' },
    }
  }

  async setEnabled(id, input) {
    if (
      !input ||
      typeof input !== 'object' ||
      Array.isArray(input) ||
      typeof input.enabled !== 'boolean' ||
      Object.keys(input).some((key) => key !== 'enabled')
    )
      throw new CustomUiError('component_request_invalid', 400)
    return this.withComponentLock(id, async () => {
      const inspected = await this.requireComponent(id)
      if (inspected.row.kind === 'legacy')
        throw new CustomUiError('component_legacy_unsupported', 409)
      if (inspected.row.kind === 'broken') throw new CustomUiError('component_invalid', 409)
      await this.state.update(id, { enabled: input.enabled })
      const next = await this.requireComponent(id)
      this.notify(id, { rev: next.row.rev ?? null, reason: 'state' })
      return { component: next.row }
    })
  }

  async deleteComponent(id) {
    if (!COMPONENT_ID_PATTERN.test(id)) throw new CustomUiError('component_not_found', 404)
    return this.withComponentLock(id, async () => {
      await this.requireComponent(id)
      const state = await this.state.get(id)
      const { instances } = await this.instanceStore.list()
      const owned = instances.filter((instance) => instance.componentId === id)
      // 先移动目录：移动失败时实例与状态都还在，不会出现“组件还在、放置没了”。
      const trashedAs = await moveComponentToTrash({
        root: this.root,
        trashRoot: this.trashRoot,
        dataRoot: this.dataRoot,
        id,
        now: this.now,
        meta: { state, instances: owned },
        ...(this.renameDirectory ? { renameDirectory: this.renameDirectory } : {}),
      })
      this.hasher.invalidate(this.componentDir(id))
      // 存储文件已随组件进了回收站：丢掉缓存，避免旧值在同名新组件里复活；内存里的日志与网络统计一并清掉。
      this.storage.forget(id)
      this.errorLog.forget(id)
      this.fetchProxy.forget(id)
      await this.instanceStore.removeForComponent(id)
      await this.state.remove(id)
      this.notify(id, { rev: null, reason: 'deleted' })
      return { ok: true, trashedAs }
    })
  }

  /**
   * 从内置模板创建：只创建，不放置，也不开会话（计划 §5.4、§5.5）。
   * 没给 id 时用模板默认 id，被占用就加 -2、-3…；明确给了 id 而被占用则报 component_already_installed。
   */
  async createFromTemplate(input) {
    if (!input || typeof input !== 'object' || Array.isArray(input))
      throw new CustomUiError('component_request_invalid', 400)
    if (Object.keys(input).some((key) => !['template', 'name', 'id'].includes(key)))
      throw new CustomUiError('component_request_invalid', 400)
    if (!COMPONENT_TEMPLATES.includes(input.template))
      throw new CustomUiError('component_template_invalid', 400)
    let name = ''
    if (input.name !== undefined) {
      if (typeof input.name !== 'string') throw new CustomUiError('component_name_invalid', 400)
      name = toWellFormed(input.name.trim())
      if (name.length > 120) throw new CustomUiError('component_name_invalid', 400)
    }
    let id
    if (input.id !== undefined) {
      if (typeof input.id !== 'string' || !COMPONENT_ID_PATTERN.test(input.id))
        throw new CustomUiError('component_id_invalid', 400)
      if (RESERVED_COMPONENT_IDS.includes(input.id) || this.builtins.has(input.id))
        throw new CustomUiError('component_id_reserved', 409)
      id = input.id
    } else {
      id = await this.availableId(
        input.template === 'blank' ? slugify(name) : templateDefaultId(input.template),
      )
    }
    const template = buildComponentTemplate(input.template, { name })
    const result = await this.writeComponent(
      { mode: 'create', id, manifest: template.manifest, files: template.files },
      { source: 'template' },
    )
    const inspected = await this.requireComponent(result.id)
    return { component: inspected.row }
  }

  async availableId(base) {
    const stem = COMPONENT_ID_PATTERN.test(base) ? base : 'component'
    for (let suffix = 1; suffix < 1000; suffix += 1) {
      const candidate = suffix === 1 ? stem : `${stem.slice(0, 60)}-${suffix}`
      if (RESERVED_COMPONENT_IDS.includes(candidate) || this.builtins.has(candidate)) continue
      if (!(await lstat(this.componentDir(candidate)).catch(() => null))) return candidate
    }
    throw new CustomUiError('component_already_installed', 409)
  }

  /**
   * 写入一个原生组件（模板创建与 S2 的 component_create 工具共用）。所有校验错误都以结构化清单返回，
   * 方便代理自己改。update 前先快照到 custom-ui-versions/<id>/<n>/。
   * @param {{ mode: 'create' | 'update', id: string, manifest?: unknown, files?: { path: string, content: string }[], deleteFiles?: string[], summary?: string }} input
   * @param {{ source?: 'agent' | 'template' | 'local', sessionId?: string }} [options]
   */
  async writeComponent(input, { source = 'agent' } = {}) {
    const { mode, id } = input || {}
    if (mode !== 'create' && mode !== 'update')
      throw new CustomUiError('component_request_invalid', 400)
    if (typeof id !== 'string' || !COMPONENT_ID_PATTERN.test(id))
      throw new CustomUiError('component_id_invalid', 400)
    if (RESERVED_COMPONENT_IDS.includes(id) || this.builtins.has(id))
      throw new CustomUiError('component_id_reserved', 409)
    const files = Array.isArray(input.files) ? input.files : []
    const deleteFiles = Array.isArray(input.deleteFiles) ? input.deleteFiles : []
    const errors = validateWriteFiles(files, deleteFiles)
    if (errors.length) throw new CustomUiError('component_files_invalid', 400, { errors })
    const summary =
      typeof input.summary === 'string' ? toWellFormed(input.summary).slice(0, 200) : ''
    return this.withComponentLock(id, async () => {
      const existing = await this.inspect(id)
      if (mode === 'create' && existing) {
        if (existing.row.kind === 'legacy')
          throw new CustomUiError('component_id_taken_by_legacy', 409)
        throw new CustomUiError('component_already_installed', 409)
      }
      if (mode === 'update' && !existing) throw new CustomUiError('component_not_found', 404)
      if (mode === 'update' && existing.row.kind === 'legacy')
        throw new CustomUiError('component_id_taken_by_legacy', 409)
      const rawManifest = input.manifest !== undefined ? input.manifest : existing?.raw
      if (mode === 'create' && input.manifest === undefined)
        throw new CustomUiError('component_manifest_invalid', 400, {
          errors: [{ path: 'manifest', code: 'manifest_missing' }],
        })
      const normalized = normalizeNativeManifest(id, rawManifest, { hostVersion: this.hostVersion })
      if (!normalized.manifest)
        throw new CustomUiError('component_manifest_invalid', 400, { errors: normalized.errors })
      const manifest = normalized.manifest
      const dir = this.componentDir(id)
      const current =
        mode === 'update' ? (await listComponentFiles(dir)).files.map((entry) => entry.path) : []
      const merged = new Set(current.filter((path) => !deleteFiles.includes(path)))
      for (const file of files) merged.add(file.path)
      merged.add('manifest.json')
      const mergedErrors = []
      if (!merged.has(manifest.entry))
        mergedErrors.push({
          path: 'entry',
          code: 'entry_missing',
          params: { entry: manifest.entry },
        })
      manifest.styles.forEach((style, index) => {
        if (!merged.has(style))
          mergedErrors.push({ path: `styles.${index}`, code: 'style_missing' })
      })
      for (const path of merged) {
        if (/\.html?$/i.test(path)) mergedErrors.push({ path, code: 'component_html_unsupported' })
      }
      if (mergedErrors.length)
        throw new CustomUiError('component_files_invalid', 400, { errors: mergedErrors })
      let snapshot = null
      if (mode === 'update') {
        snapshot = await snapshotComponent({
          versionsRoot: this.versionsRoot,
          id,
          dir,
          now: this.now,
          summary,
        })
      }
      const writes = [
        ...files,
        // manifest 落盘的是调用方给的原文（已校验），保留作者自己的字段与格式意图。
        ...(input.manifest !== undefined
          ? [{ path: 'manifest.json', content: `${JSON.stringify(input.manifest, null, 2)}\n` }]
          : []),
      ]
      let previous
      try {
        ;({ previous } = await writeComponentDirectory({
          root: this.root,
          id,
          mode,
          files: writes,
          deleteFiles,
          ...(this.renameDirectory ? { renameDirectory: this.renameDirectory } : {}),
        }))
      } catch (error) {
        if (error?.code === 'EEXIST') throw new CustomUiError('component_already_installed', 409)
        if (error?.code === 'ENOENT') throw new CustomUiError('component_not_found', 404)
        throw error
      }
      this.hasher.invalidate(dir)
      if (mode === 'create') await this.state.update(id, { enabled: true, source })
      else if (source === 'agent') await this.state.update(id, { enabled: true, source })
      const written = await this.inspect(id)
      const rev = written?.row.rev ?? null
      this.notify(
        id,
        { rev, reason: mode === 'create' ? 'created' : 'files' },
        written ? changeKey(written.row) : undefined,
      )
      return {
        id,
        mode,
        version: manifest.version,
        rev,
        status: written?.row.status || 'invalid',
        files: writes.map((file) => ({
          path: file.path,
          bytes: Buffer.byteLength(file.content),
          ...lineDelta(previous.get(file.path), file.content),
        })),
        deleted: deleteFiles.filter((path) => previous.has(path)),
        snapshot: snapshot?.n ?? null,
      }
    })
  }

  /**
   * 原生模块与资源：GET /api/custom-ui/components/:id/r/:rev/<path>。
   * 走正常鉴权；没有 sandbox CSP、没有 ACAO；rev 等于当前哈希时 immutable，否则 no-store 但照样返回当前内容。
   */
  async serveModuleAsset({ id, rev, segments, res }) {
    if (!COMPONENT_ID_PATTERN.test(id)) throw new CustomUiError('component_not_found', 404)
    const path = segments.join('/')
    if (
      !/^[a-f0-9]{24}$/.test(rev) ||
      segments.some((segment) => segment.includes('/') || segment.includes('\\')) ||
      !isSafeComponentPath(path) ||
      path === 'manifest.json' ||
      !componentFileType(path)
    )
      throw new CustomUiError('component_asset_missing', 404)
    const inspected = await this.requireComponent(id)
    if (inspected.row.kind === 'legacy')
      throw new CustomUiError('component_legacy_unsupported', 409)
    if (inspected.row.status !== 'ready') throw new CustomUiError('component_invalid', 409)
    const target = await resolveComponentFile(this.root, id, path, { maxBytes: MAX_ASSET_BYTES })
    const link = target ? await lstat(target.file).catch(() => null) : null
    // 哈希遍历不跟随符号链接，模块路由也不服务它们，否则内容变化不会反映到 rev 上。
    if (!target || !link?.isFile()) throw new CustomUiError('component_asset_missing', 404)
    const type = componentFileType(path)
    const headers = {
      'Content-Type': type,
      'Content-Length': target.size,
      'X-Content-Type-Options': 'nosniff',
      'Cross-Origin-Resource-Policy': 'same-origin',
      'Cache-Control':
        rev === inspected.row.rev ? 'private, max-age=31536000, immutable' : 'no-store',
      ...(extname(path).toLowerCase() === '.svg'
        ? {
            // 收紧而不是放宽：直接打开组件里的 SVG 也不能执行脚本。
            'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'; sandbox",
          }
        : {}),
    }
    await new Promise((resolveStream) => {
      const stream = createReadStream(target.file)
      stream.once('open', () => {
        res.writeHead(200, headers)
        stream.pipe(res)
      })
      stream.once('error', () => {
        if (!res.headersSent) {
          res.writeHead(404, { 'Content-Type': 'application/json; charset=utf-8' })
          res.end(
            JSON.stringify({ code: 'component_asset_missing', error: 'component_asset_missing' }),
          )
        } else res.destroy()
        resolveStream()
      })
      stream.once('end', resolveStream)
      res.once('close', () => {
        stream.destroy()
        resolveStream()
      })
    })
  }

  // ── 运行期能力：存储、网络代理、错误日志（S2）──

  /**
   * 能力接口的组件前提：原生且可运行（ready）。停用的组件仍可在工作室预览，所以存储不要求 enabled；
   * 网络代理按计划 §3.7 另外要求 enabled。
   * @param {string} id
   */
  async runnableComponent(id) {
    if (typeof id !== 'string' || !COMPONENT_ID_PATTERN.test(id))
      throw new CustomUiError('component_not_found', 404)
    const inspected = await this.requireComponent(id)
    if (inspected.row.kind === 'legacy')
      throw new CustomUiError('component_legacy_unsupported', 409)
    if (!inspected.manifest) throw new CustomUiError('component_invalid', 409)
    return { row: inspected.row, manifest: inspected.manifest }
  }

  /** @param {string} id */
  async storageQuota(id) {
    const { manifest } = await this.runnableComponent(id)
    const storage = manifest.capabilities.storage
    if (!storage)
      throw new CustomUiError('component_capability_denied', 403, { capability: 'storage' })
    return storage.quotaKb * 1024
  }

  /** @param {string} id @param {unknown} key */
  async storageGet(id, key) {
    await this.storageQuota(id)
    return this.storage.get(id, key)
  }

  /** @param {string} id @param {unknown} key @param {unknown} input */
  async storageSet(id, key, input) {
    const quotaBytes = await this.storageQuota(id)
    return this.storage.set(id, key, input, quotaBytes)
  }

  /** @param {string} id @param {unknown} key */
  async storageRemove(id, key) {
    await this.storageQuota(id)
    return this.storage.remove(id, key)
  }

  /** @param {string} id @param {unknown} input */
  async fetchForComponent(id, input) {
    const { row, manifest } = await this.runnableComponent(id)
    if (!row.enabled) throw new CustomUiError('component_disabled', 409)
    return this.fetchProxy.fetch(id, manifest.capabilities.network, input)
  }

  /** @param {string} id @param {unknown} input */
  async reportError(id, input) {
    if (typeof id !== 'string' || !COMPONENT_ID_PATTERN.test(id))
      throw new CustomUiError('component_not_found', 404)
    await this.requireComponent(id)
    return this.errorLog.record(id, input)
  }

  /** @param {string} id */
  async listErrors(id) {
    if (typeof id !== 'string' || !COMPONENT_ID_PATTERN.test(id))
      throw new CustomUiError('component_not_found', 404)
    await this.requireComponent(id)
    return { ...this.errorLog.list(id), network: this.fetchProxy.networkStats(id) }
  }

  /**
   * 工作室“生成”的绑定会话（计划 §5.4）：状态里记着的会话还在且没归档就复用，否则新建
   * “生成组件 · <名称>”会话（cwd 是组件目录）并记下 id。会话的创建与查询由调用方注入，领域规则留在这里。
   * @param {string} id
   * @param {{
   *   createSession: (name: string, cwd: string) => Promise<{ id: string }>,
   *   findSession: (sessionId: string) => Promise<{ archived?: boolean } | null>,
   * }} sessions
   */
  async ensureAgentSession(id, { createSession, findSession }) {
    if (typeof id !== 'string' || !COMPONENT_ID_PATTERN.test(id))
      throw new CustomUiError('component_not_found', 404)
    return this.withComponentLock(id, async () => {
      const inspected = await this.requireComponent(id)
      if (inspected.row.kind === 'legacy')
        throw new CustomUiError('component_legacy_unsupported', 409)
      if (inspected.row.kind === 'broken') throw new CustomUiError('component_invalid', 409)
      const state = await this.state.get(id)
      const bound = state?.boundSessionId
      if (typeof bound === 'string' && bound) {
        const session = await findSession(bound).catch(() => null)
        if (session && session.archived !== true)
          return { sessionId: bound, created: false, command: '/skill:pisper-component' }
      }
      const created = await createSession(`生成组件 · ${inspected.row.name}`, this.componentDir(id))
      if (!created || typeof created.id !== 'string' || !created.id)
        throw new Error('component session was not created')
      await this.state.update(id, { boundSessionId: created.id })
      return { sessionId: created.id, created: true, command: '/skill:pisper-component' }
    })
  }

  // ── 放置实例 ──

  async listInstances(surface) {
    if (surface !== undefined && surface !== null && surface !== '' && !isComponentSurface(surface))
      throw new CustomUiError('component_surface_invalid', 400)
    return this.instanceStore.list(surface || undefined)
  }

  /** 组件层面的前提：native、ready、enabled，且 manifest 声明了这个放置位置。 */
  async placeableComponent(componentId, surface, { requireEnabled = true } = {}) {
    if (typeof componentId !== 'string' || !COMPONENT_ID_PATTERN.test(componentId))
      throw new CustomUiError('component_id_invalid', 400)
    const inspected = await this.requireComponent(componentId)
    if (inspected.row.kind === 'legacy')
      throw new CustomUiError('component_legacy_unsupported', 409)
    if (!inspected.manifest) throw new CustomUiError('component_invalid', 409)
    if (requireEnabled && !inspected.row.enabled) throw new CustomUiError('component_disabled', 409)
    const placement = manifestPlacement(inspected.manifest, surface)
    if (!placement) throw new CustomUiError('component_surface_unsupported', 400, { surface })
    return { manifest: inspected.manifest, placement }
  }

  async createInstance(input) {
    if (!isPlainObject(input)) throw new CustomUiError('component_request_invalid', 400)
    const allowed = ['componentId', 'surface', 'size', 'scope', 'title', 'props']
    if (Object.keys(input).some((key) => !allowed.includes(key)))
      throw new CustomUiError('component_request_invalid', 400)
    if (!isComponentSurface(input.surface))
      throw new CustomUiError('component_surface_invalid', 400)
    const surface = input.surface
    const { manifest, placement } = await this.placeableComponent(input.componentId, surface)
    const fields = parseInstanceFields(input, surface, placement, manifest, { creating: true })
    const result = await this.instanceStore.create({
      componentId: input.componentId,
      surface,
      size: fields.size ?? null,
      scope: fields.scope ?? null,
      title: fields.title ?? '',
      props: fields.props ?? {},
    })
    this.notify(input.componentId, { reason: 'instances' })
    return result
  }

  async updateInstance(instanceId, patch) {
    if (!isPlainObject(patch) || !Object.keys(patch).length)
      throw new CustomUiError('component_request_invalid', 400)
    const allowed = ['size', 'scope', 'title', 'props', 'enabled']
    if (Object.keys(patch).some((key) => !allowed.includes(key)))
      throw new CustomUiError('component_request_invalid', 400)
    const current = await this.instanceStore.get(instanceId)
    if (!current) throw new CustomUiError('component_instance_not_found', 404)
    let manifest = null
    let placement = null
    if (patch.size !== undefined || patch.scope !== undefined || patch.props !== undefined) {
      // 停用的组件仍可改属性（工作室里停用后再调参数）；只有不能运行的组件不能校验属性。
      ;({ manifest, placement } = await this.placeableComponent(
        current.componentId,
        current.surface,
        {
          requireEnabled: false,
        },
      ))
    }
    const fields = parseInstanceFields(patch, current.surface, placement, manifest, {
      creating: false,
    })
    if (patch.enabled !== undefined) {
      if (typeof patch.enabled !== 'boolean')
        throw new CustomUiError('component_request_invalid', 400)
      fields.enabled = patch.enabled
    }
    const result = await this.instanceStore.update(instanceId, fields)
    this.notify(current.componentId, { reason: 'instances' })
    return result
  }

  async removeInstance(instanceId) {
    const { revision, instance } = await this.instanceStore.remove(instanceId)
    this.notify(instance.componentId, { reason: 'instances' })
    return { revision }
  }

  async reorderInstances(input) {
    if (
      !isPlainObject(input) ||
      Object.keys(input).some((key) => !['baseRevision', 'surface', 'order'].includes(key)) ||
      !Number.isSafeInteger(input.baseRevision) ||
      !Array.isArray(input.order) ||
      input.order.length > 200 ||
      input.order.some((id) => typeof id !== 'string')
    )
      throw new CustomUiError('component_request_invalid', 400)
    if (!isComponentSurface(input.surface))
      throw new CustomUiError('component_surface_invalid', 400)
    const result = await this.instanceStore.reorder(input)
    for (const componentId of new Set(result.instances.map((instance) => instance.componentId)))
      this.notify(componentId, { reason: 'instances' })
    return result
  }
}

// ── 原生组件的输入校验辅助（纯函数，按不可信 unknown 处理）──

/**
 * 去重用的内容键：ready 时就是 rev；否则是状态加错误清单（错误变了也要通知工作室）。
 * @param {{ rev?: string, status: string, errors: unknown[] }} row
 */
function changeKey(row) {
  return row.rev || `${row.status}:${JSON.stringify(row.errors)}`
}

const WRITE_EXTENSIONS = new Set(['.js', '.mjs', '.css', '.json', '.svg'])
const MAX_WRITE_FILES = 32
const MAX_WRITE_FILE_BYTES = 256 * 1024
const MAX_WRITE_TOTAL_BYTES = 1024 * 1024
const MAX_DELETE_FILES = 64
const STATIC_IMPORT =
  /(?:^|[;\n}])\s*(?:import|export)\s+(?:[^'"`;]*?\s+from\s+)?(['"])([^'"\n]+)\1/g
const DYNAMIC_IMPORT = /\bimport\s*\(\s*(['"`])([^'"`\n]+)\1\s*\)/g

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

// 中文等非 ASCII 名称没有可用的 slug，退回 component（再按 -2、-3 去重）。
function slugify(name) {
  const slug = String(name || '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 48)
    .replace(/-+$/, '')
  return COMPONENT_ID_PATTERN.test(slug) ? slug : 'component'
}

/** 找出裸模块导入：宿主不提供模块解析，组件只能用 ./、../ 或 / 开头的相对导入。 */
export function findBareImports(source) {
  const specifiers = []
  for (const pattern of [STATIC_IMPORT, DYNAMIC_IMPORT]) {
    pattern.lastIndex = 0
    for (const match of source.matchAll(pattern)) {
      const specifier = match[2]
      if (!/^(?:\.\.?\/|\/)/.test(specifier)) specifiers.push(specifier)
    }
  }
  return [...new Set(specifiers)]
}

function validateWriteFiles(files, deleteFiles) {
  const errors = []
  if (files.length > MAX_WRITE_FILES)
    errors.push({
      path: 'files',
      code: 'component_files_too_many',
      params: { max: MAX_WRITE_FILES },
    })
  if (deleteFiles.length > MAX_DELETE_FILES)
    errors.push({
      path: 'deleteFiles',
      code: 'component_files_too_many',
      params: { max: MAX_DELETE_FILES },
    })
  let total = 0
  const seen = new Set()
  files.slice(0, MAX_WRITE_FILES).forEach((file, index) => {
    const path = isPlainObject(file) ? file.path : undefined
    if (
      !isSafeComponentPath(path) ||
      path === 'manifest.json' ||
      !WRITE_EXTENSIONS.has(extname(path).toLowerCase())
    ) {
      errors.push({ path: `files.${index}.path`, code: 'component_file_path_invalid' })
      return
    }
    if (seen.has(path)) {
      errors.push({ path: `files.${index}.path`, code: 'component_file_duplicate' })
      return
    }
    seen.add(path)
    if (typeof file.content !== 'string') {
      errors.push({ path: `files.${index}.content`, code: 'component_file_content_invalid' })
      return
    }
    const bytes = Buffer.byteLength(file.content)
    total += bytes
    if (bytes > MAX_WRITE_FILE_BYTES)
      errors.push({
        path: `files.${index}.content`,
        code: 'component_file_too_large',
        params: { max: MAX_WRITE_FILE_BYTES },
      })
    if (/\.m?js$/i.test(path)) {
      for (const specifier of findBareImports(file.content))
        errors.push({
          path: `files.${index}.content`,
          code: 'component_bare_import',
          params: { specifier },
        })
    }
  })
  if (total > MAX_WRITE_TOTAL_BYTES)
    errors.push({
      path: 'files',
      code: 'component_files_too_large',
      params: { max: MAX_WRITE_TOTAL_BYTES },
    })
  deleteFiles.slice(0, MAX_DELETE_FILES).forEach((path, index) => {
    if (!isSafeComponentPath(path) || path === 'manifest.json')
      errors.push({ path: `deleteFiles.${index}`, code: 'component_file_path_invalid' })
  })
  return errors
}

// 简单行差：按行做多重集合比较，给出新增与删除的行数（改动芯片用，不是精确 diff）。
function lineDelta(previous, content) {
  const before = previous ? previous.toString('utf8').split('\n') : []
  const after = content.split('\n')
  const counts = new Map()
  for (const line of before) counts.set(line, (counts.get(line) || 0) + 1)
  let added = 0
  for (const line of after) {
    const count = counts.get(line) || 0
    if (count) counts.set(line, count - 1)
    else added += 1
  }
  let removed = 0
  for (const count of counts.values()) removed += count
  return { added, removed }
}

// 实例字段：size 只对 overview-card、scope 只对 session-panel 有效；props 按 manifest 的 schema 校验。
function parseInstanceFields(input, surface, placement, manifest, { creating }) {
  const fields = {}
  if (input.size !== undefined && input.size !== null) {
    if (surface !== 'overview-card' || !placement?.sizes?.includes(input.size))
      throw new CustomUiError('component_size_invalid', 400)
    fields.size = input.size
  } else if (input.size === null && surface === 'overview-card') {
    throw new CustomUiError('component_size_invalid', 400)
  } else if (creating && surface === 'overview-card') {
    fields.size = placement.sizes[0]
  }
  if (input.scope !== undefined && input.scope !== null) {
    const scope = surface === 'session-panel' ? parseInstanceScope(input.scope) : null
    if (!scope) throw new CustomUiError('component_scope_invalid', 400)
    fields.scope = scope
  } else if (surface === 'session-panel' && (creating || input.scope === null)) {
    fields.scope = { type: 'all' }
  }
  if (input.title !== undefined) {
    const title = parseInstanceTitle(input.title)
    if (title === null) throw new CustomUiError('component_title_invalid', 400)
    fields.title = title
  }
  if (input.props !== undefined || creating) {
    const checked = validateComponentProps(manifest.props, input.props)
    if (checked.errors.length)
      throw new CustomUiError('component_props_invalid', 400, { errors: checked.errors })
    fields.props = checked.value
  }
  return fields
}
