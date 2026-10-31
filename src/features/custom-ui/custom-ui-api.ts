// 原生组件（v2）的类型与 API 封装：与 runtime/services/custom-ui-service.mjs 的输出保持契约一致。
// 目录来自 Runtime；用户主目录按服务端平台缩写为 ~ 或 %USERPROFILE%。旧 iframe 组件的视图凭证、
// 导入与旧列表形状在 A7 S4 随旧机制一起删除。
import type {
  ComponentSurface,
  ManifestCapabilities,
  ManifestPlacement,
  NativeManifest,
  OverviewCardSize,
  PropsSchema,
} from '@shared/custom-ui-manifest.mjs'
import { ApiError, requestJson, requestText } from '@/lib/http'
import type { SessionSummary } from '@/types/chat'

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

// ---------------------------------------------------------------------------
// v2 原生组件协议（计划 §3.4）。解析器同时接受旧 Runtime 的列表形状：
// 没有 kind 的行一律视为 legacy（旧 Runtime 只有 iframe 组件），不会被当成可运行的原生组件。
// 本节只做形状校验与归一化（Runtime 已规范化过 manifest），不导入 shared/custom-ui-manifest.mjs 的
// 运行时代码，好让轻量入口 placements.ts 可以静态导入这里。
// ---------------------------------------------------------------------------

export type ComponentKind = 'native' | 'legacy' | 'broken'
export type ComponentStatus = 'ready' | 'invalid' | 'unsupported'
export type ComponentSource = 'agent' | 'template' | 'local'
export type ComponentFieldError = {
  path: string
  code: string
  params?: Record<string, string | number>
}

export type ComponentRow = {
  id: string
  kind: ComponentKind
  status: ComponentStatus
  name: string
  version: string
  description: string
  directory: string
  enabled: boolean
  source: ComponentSource
  // 只在 status === 'ready' 时有。
  rev?: string
  entry: string
  // 只在原生且 manifest 没有错误时有。
  manifest?: NativeManifest
  errors: ComponentFieldError[]
  updatedAt: string
}

export type ComponentCatalogFeatures = {
  import: boolean
  versions: boolean
  export: boolean
  // 'none'：旧 Runtime 没有 DELETE（兼容矩阵 §3.12），界面隐藏删除按钮。
  trash: 'move' | 'none'
}

export type ComponentCatalog = {
  root: string
  features: ComponentCatalogFeatures
  components: ComponentRow[]
  // 列表是旧形状（第 2–4 期 Runtime）：全部按 legacy 显示，宿主不加载任何组件。
  legacyShape: boolean
}

export type ComponentInstanceScope = { type: 'all' } | { type: 'project'; cwd: string }

export type ComponentInstance = {
  instanceId: string
  componentId: string
  surface: ComponentSurface
  size: OverviewCardSize | null
  scope: ComponentInstanceScope | null
  title: string
  props: Record<string, unknown>
  enabled: boolean
  order: number
  createdAt: string
  updatedAt: string
}

export type ComponentInstancesData = { revision: number; instances: ComponentInstance[] }

export type ComponentErrorEntry = {
  at: string
  kind: string
  level: string
  message: string
  stack?: string
  instanceId?: string
  surface?: string
}

export type ComponentErrorsData = {
  entries: ComponentErrorEntry[]
  dropped: number
  network: { count: number; lastAt: string | null; hosts: Record<string, number> }
}

export type ComponentDescription = {
  component: ComponentRow
  files: { path: string; size: number; type: string }[]
  errors: ComponentFieldError[]
  scan: { status: string }
}

export type ComponentReportKind = 'load' | 'render' | 'callback' | 'slow' | 'log'
export type ComponentReport = {
  instanceId?: string
  surface?: ComponentSurface
  kind: ComponentReportKind
  level: 'error' | 'warn'
  message: string
  stack?: string
}

export type ComponentFetchRequest = {
  url: string
  method?: string
  headers?: Record<string, string>
  body?: string
  bodyEncoding?: 'utf8' | 'base64'
}

export type ComponentFetchResult = {
  status: number
  statusText: string
  headers: Record<string, string>
  body: string
  bodyEncoding: 'utf8' | 'base64'
  url: string
  redirected: boolean
}

const SURFACES: readonly ComponentSurface[] = [
  'overview-card',
  'session-panel',
  'feature-page',
  'floating',
]
const CARD_SIZES: readonly OverviewCardSize[] = ['S', 'M']
const COMPONENT_ID = /^[a-z0-9][a-z0-9._-]{0,63}$/
const COMPONENT_REV = /^[a-f0-9]{24}$/
// 与 Runtime 实例 id 规则一致：浮窗位置以它作节点 id（floating-placement.ts 的键规则）。
const INSTANCE_ID = /^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,79}$/
const PATH_SEGMENT = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/

function text(value: unknown, max = 4000): string {
  return typeof value === 'string' ? value.slice(0, max) : ''
}

function isSafeModulePath(value: unknown): value is string {
  if (typeof value !== 'string' || !value || value.length > 512) return false
  const segments = value.split('/')
  return segments.length <= 3 && segments.every((segment) => PATH_SEGMENT.test(segment))
}

function stringRecord(value: unknown, max = 200): Record<string, string> {
  const result: Record<string, string> = {}
  if (!isRecord(value)) return result
  for (const [key, item] of Object.entries(value).slice(0, max))
    if (typeof item === 'string') result[key] = item
  return result
}

function parseFieldErrors(value: unknown): ComponentFieldError[] {
  if (!Array.isArray(value)) return []
  const errors: ComponentFieldError[] = []
  for (const item of value.slice(0, 200)) {
    if (!isRecord(item) || typeof item.code !== 'string' || !item.code) continue
    const entry: ComponentFieldError = { path: text(item.path, 512), code: text(item.code, 128) }
    if (isRecord(item.params)) {
      const params: Record<string, string | number> = {}
      for (const [key, param] of Object.entries(item.params))
        if (typeof param === 'string' || (typeof param === 'number' && Number.isFinite(param)))
          params[key] = param
      entry.params = params
    }
    errors.push(entry)
  }
  return errors
}

function parseManifestLocales(value: unknown): NativeManifest['i18n'] {
  const result: NativeManifest['i18n'] = {}
  if (!isRecord(value)) return result
  for (const locale of ['zh-CN', 'en-US'] as const) {
    const entry = value[locale]
    if (!isRecord(entry)) continue
    result[locale] = {
      ...(typeof entry.name === 'string' ? { name: entry.name } : {}),
      ...(typeof entry.description === 'string' ? { description: entry.description } : {}),
      strings: stringRecord(entry.strings, 500),
    }
  }
  return result
}

function parseManifestPlacements(value: unknown): ManifestPlacement[] {
  if (!Array.isArray(value)) return []
  const placements: ManifestPlacement[] = []
  for (const item of value) {
    if (!isRecord(item) || !SURFACES.includes(item.surface as ComponentSurface)) continue
    const surface = item.surface as ComponentSurface
    if (placements.some((entry) => entry.surface === surface)) continue
    const placement: ManifestPlacement = { surface }
    if (Array.isArray(item.sizes)) {
      const sizes = CARD_SIZES.filter((size) => (item.sizes as unknown[]).includes(size))
      if (sizes.length) placement.sizes = sizes
    }
    if (
      isRecord(item.size) &&
      typeof item.size.w === 'number' &&
      typeof item.size.h === 'number' &&
      Number.isFinite(item.size.w) &&
      Number.isFinite(item.size.h) &&
      item.size.w > 0 &&
      item.size.h > 0
    )
      placement.size = { w: item.size.w, h: item.size.h }
    placements.push(placement)
  }
  return placements
}

function parseManifestCapabilities(value: unknown): ManifestCapabilities {
  const result: ManifestCapabilities = {}
  if (!isRecord(value)) return result
  const reason = (entry: Record<string, unknown>) => text(entry.reason, 500)
  for (const key of ['notify', 'sessions.read', 'sessions.open'] as const) {
    const entry = value[key]
    if (isRecord(entry)) result[key] = { reason: reason(entry) }
  }
  const network = value.network
  if (isRecord(network) && Array.isArray(network.hosts))
    result.network = {
      reason: reason(network),
      hosts: network.hosts.filter((host): host is string => typeof host === 'string').slice(0, 32),
    }
  const storage = value.storage
  if (isRecord(storage))
    result.storage = {
      reason: reason(storage),
      quotaKb:
        typeof storage.quotaKb === 'number' && Number.isFinite(storage.quotaKb)
          ? storage.quotaKb
          : 256,
    }
  return result
}

/** Runtime 已规范化的 manifest 的形状校验；核心字段不合格时返回 null（整行按 invalid 处理）。 */
export function parseNativeManifest(id: string, value: unknown): NativeManifest | null {
  if (!isRecord(value) || value.runtime !== 'native' || value.id !== id) return null
  if (!isSafeModulePath(value.entry)) return null
  const props = isRecord(value.props) ? value.props : null
  if (!props || props.type !== 'object' || !isRecord(props.properties)) return null
  const styles = Array.isArray(value.styles) ? value.styles : []
  if (!styles.every(isSafeModulePath)) return null
  return {
    runtime: 'native',
    id,
    name: text(value.name, 120) || id,
    version: text(value.version, 64),
    description: text(value.description, 500),
    entry: value.entry,
    styles: styles.slice(0, 8),
    icon: text(value.icon, 48),
    i18n: parseManifestLocales(value.i18n),
    // 属性字段的逐项校验在 Runtime（写入与 PATCH 都会校验）；表单（S5）按 shared 的规则再规范化。
    props: {
      type: 'object',
      properties: props.properties as PropsSchema['properties'],
      required: Array.isArray(props.required)
        ? props.required.filter((key): key is string => typeof key === 'string')
        : [],
    },
    placements: parseManifestPlacements(value.placements),
    capabilities: parseManifestCapabilities(value.capabilities),
    sdk: typeof value.sdk === 'number' ? value.sdk : 1,
    minHostVersion: text(value.minHostVersion, 64),
  }
}

function parseSource(value: unknown): ComponentSource {
  return value === 'agent' || value === 'template' ? value : 'local'
}

/** 解析一行；旧形状（没有 kind）按 legacy。id 不合法的行丢弃（返回 null）。 */
export function parseComponentRow(value: unknown): ComponentRow | null {
  if (!isRecord(value) || typeof value.id !== 'string' || !COMPONENT_ID.test(value.id)) return null
  const id = value.id
  const base = {
    id,
    name: text(value.name, 120) || id,
    version: text(value.version, 64),
    description: text(value.description, 500),
    directory: text(value.directory, 4096),
    updatedAt: text(value.updatedAt, 64),
  }
  if (value.kind !== 'native' && value.kind !== 'broken') {
    // 旧形状与 kind:'legacy'：不可运行，只能删除（新 Runtime）或忽略（旧 Runtime）。
    return {
      ...base,
      kind: 'legacy',
      status: 'unsupported',
      enabled: false,
      source: parseSource(value.source),
      entry: '',
      errors: parseFieldErrors(value.errors),
    }
  }
  if (value.kind === 'broken')
    return {
      ...base,
      kind: 'broken',
      status: 'invalid',
      enabled: false,
      source: parseSource(value.source),
      entry: '',
      errors: parseFieldErrors(value.errors),
    }
  const errors = parseFieldErrors(value.errors)
  const row: ComponentRow = {
    ...base,
    kind: 'native',
    status: value.status === 'ready' ? 'ready' : 'invalid',
    enabled: value.enabled === true,
    source: parseSource(value.source),
    entry: text(value.entry, 512),
    errors,
  }
  if (value.manifest !== undefined) {
    const manifest = parseNativeManifest(id, value.manifest)
    if (manifest) row.manifest = manifest
  }
  if (row.status === 'ready') {
    // ready 行必须同时有合法的 rev 与 manifest，否则不能加载：按 invalid 显示，不冒充可用。
    if (typeof value.rev === 'string' && COMPONENT_REV.test(value.rev) && row.manifest)
      row.rev = value.rev
    else {
      row.status = 'invalid'
      row.errors = [...errors, { path: '', code: 'component_response_invalid' }]
    }
  }
  return row
}

export function parseComponentCatalog(value: unknown): ComponentCatalog {
  if (!isRecord(value) || typeof value.root !== 'string' || !Array.isArray(value.components))
    throw new Error('Invalid custom UI component response')
  const features = isRecord(value.features) ? value.features : null
  const components: ComponentRow[] = []
  const seen = new Set<string>()
  for (const item of value.components) {
    const row = parseComponentRow(item)
    if (!row || seen.has(row.id)) continue
    seen.add(row.id)
    components.push(row)
  }
  return {
    root: value.root,
    features: {
      import: features?.import === true,
      versions: features?.versions === true,
      export: features?.export === true,
      trash: features?.trash === 'move' ? 'move' : 'none',
    },
    components,
    legacyShape: !features,
  }
}

function parseScope(value: unknown): ComponentInstanceScope | null {
  if (!isRecord(value)) return null
  if (value.type === 'all') return { type: 'all' }
  if (value.type === 'project' && typeof value.cwd === 'string' && value.cwd)
    return { type: 'project', cwd: value.cwd }
  return null
}

export function parseComponentInstance(value: unknown): ComponentInstance | null {
  if (
    !isRecord(value) ||
    typeof value.instanceId !== 'string' ||
    !INSTANCE_ID.test(value.instanceId) ||
    typeof value.componentId !== 'string' ||
    !COMPONENT_ID.test(value.componentId) ||
    !SURFACES.includes(value.surface as ComponentSurface)
  )
    return null
  const surface = value.surface as ComponentSurface
  return {
    instanceId: value.instanceId,
    componentId: value.componentId,
    surface,
    size:
      surface === 'overview-card'
        ? CARD_SIZES.includes(value.size as OverviewCardSize)
          ? (value.size as OverviewCardSize)
          : 'S'
        : null,
    scope: surface === 'session-panel' ? (parseScope(value.scope) ?? { type: 'all' }) : null,
    title: text(value.title, 80),
    props: isRecord(value.props) ? value.props : {},
    enabled: value.enabled !== false,
    order: typeof value.order === 'number' && Number.isFinite(value.order) ? value.order : 0,
    createdAt: text(value.createdAt, 64),
    updatedAt: text(value.updatedAt, 64),
  }
}

export function parseComponentInstances(value: unknown): ComponentInstancesData {
  if (
    !isRecord(value) ||
    typeof value.revision !== 'number' ||
    !Number.isSafeInteger(value.revision) ||
    !Array.isArray(value.instances)
  )
    throw new Error('Invalid custom UI instances response')
  const instances: ComponentInstance[] = []
  const seen = new Set<string>()
  for (const item of value.instances) {
    const instance = parseComponentInstance(item)
    if (!instance || seen.has(instance.instanceId)) continue
    seen.add(instance.instanceId)
    instances.push(instance)
  }
  instances.sort((left, right) => left.order - right.order)
  return { revision: value.revision, instances }
}

function parseInstanceResponse(value: unknown): { revision: number; instance: ComponentInstance } {
  const instance = isRecord(value) ? parseComponentInstance(value.instance) : null
  if (!instance || !isRecord(value) || typeof value.revision !== 'number')
    throw new Error('Invalid custom UI instance response')
  return { revision: value.revision, instance }
}

export function parseComponentErrors(value: unknown): ComponentErrorsData {
  if (!isRecord(value) || !Array.isArray(value.entries))
    throw new Error('Invalid custom UI errors response')
  const entries: ComponentErrorEntry[] = []
  for (const item of value.entries.slice(-200)) {
    if (!isRecord(item) || typeof item.message !== 'string') continue
    entries.push({
      at: text(item.at, 64),
      kind: text(item.kind, 32),
      level: text(item.level, 16),
      message: text(item.message, 500),
      ...(typeof item.stack === 'string' ? { stack: text(item.stack, 4000) } : {}),
      ...(typeof item.instanceId === 'string' ? { instanceId: text(item.instanceId, 80) } : {}),
      ...(typeof item.surface === 'string' ? { surface: text(item.surface, 32) } : {}),
    })
  }
  const network = isRecord(value.network) ? value.network : {}
  const hosts: Record<string, number> = {}
  if (isRecord(network.hosts))
    for (const [host, count] of Object.entries(network.hosts).slice(0, 64))
      if (typeof count === 'number' && Number.isFinite(count)) hosts[host] = count
  return {
    entries,
    dropped:
      typeof value.dropped === 'number' && Number.isFinite(value.dropped) ? value.dropped : 0,
    network: {
      count:
        typeof network.count === 'number' && Number.isFinite(network.count) ? network.count : 0,
      lastAt: typeof network.lastAt === 'string' ? network.lastAt : null,
      hosts,
    },
  }
}

export function parseComponentDescription(value: unknown): ComponentDescription {
  const component = isRecord(value) ? parseComponentRow(value.component) : null
  if (!component || !isRecord(value) || !Array.isArray(value.files))
    throw new Error('Invalid custom UI describe response')
  return {
    component,
    files: value.files
      .filter(
        (file): file is { path: string; size: number; type: string } =>
          isRecord(file) &&
          typeof file.path === 'string' &&
          typeof file.size === 'number' &&
          typeof file.type === 'string',
      )
      .map((file) => ({ path: file.path, size: file.size, type: file.type })),
    errors: parseFieldErrors(value.errors),
    scan: { status: isRecord(value.scan) ? text(value.scan.status, 32) : 'unsupported' },
  }
}

export function parseComponentFetchResult(value: unknown): ComponentFetchResult {
  if (
    !isRecord(value) ||
    typeof value.status !== 'number' ||
    typeof value.body !== 'string' ||
    (value.bodyEncoding !== 'utf8' && value.bodyEncoding !== 'base64')
  )
    throw new Error('Invalid custom UI fetch response')
  return {
    status: value.status,
    statusText: text(value.statusText, 200),
    headers: stringRecord(value.headers, 64),
    body: value.body,
    bodyEncoding: value.bodyEncoding,
    url: text(value.url, 2048),
    redirected: value.redirected === true,
  }
}

/** ApiError 的错误体是 {code, error: code, ...}；取出组件错误码，其他错误返回 null。 */
export function componentErrorCode(error: unknown): string | null {
  if (!(error instanceof ApiError) || !error.data) return null
  const code = error.data.code ?? error.data.error
  return typeof code === 'string' && /^[a-z0-9_]{1,64}$/.test(code) ? code : null
}

/** 带字段级清单的错误（component_props_invalid 等）：取出 errors，非这类错误返回空数组。 */
export function componentFieldErrors(error: unknown): ComponentFieldError[] {
  if (!(error instanceof ApiError) || !error.data) return []
  return parseFieldErrors(error.data.errors)
}

const componentPath = (id: string) => `/api/custom-ui/components/${encodeURIComponent(id)}`

/** 模块 URL：rev 放在路径里，组件内相对导入与样式落在同一个 r/<rev>/ 前缀下（计划 §3.6）。 */
export function componentModuleUrl(id: string, rev: string, path: string): string {
  if (!COMPONENT_ID.test(id) || !COMPONENT_REV.test(rev) || !isSafeModulePath(path))
    throw new Error('Invalid component module path')
  return `${componentPath(id)}/r/${rev}/${path
    .split('/')
    .map((segment) => encodeURIComponent(segment))
    .join('/')}`
}

/** 工作室源码页：按 rev 路由读一个文件的文本（manifest.json 不经模块路由服务，界面改显示规范化后的 manifest）。 */
export function readComponentSource(id: string, rev: string, path: string, signal?: AbortSignal) {
  return requestText(componentModuleUrl(id, rev, path), { signal })
}

export function listComponentCatalog(signal?: AbortSignal): Promise<ComponentCatalog> {
  return requestJson('/api/custom-ui/components', { signal, parse: parseComponentCatalog })
}

export function describeComponent(id: string, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/describe`, {
    signal,
    parse: parseComponentDescription,
  })
}

export function createComponentFromTemplate(input: {
  template: 'blank' | 'usage-today' | 'notes'
  name?: string
  id?: string
}): Promise<ComponentRow> {
  return requestJson('/api/custom-ui/components', {
    method: 'POST',
    body: input,
    parse: (value) => {
      const row = isRecord(value) ? parseComponentRow(value.component) : null
      if (!row) throw new Error('Invalid custom UI component response')
      return row
    },
  })
}

export function setComponentEnabled(id: string, enabled: boolean): Promise<ComponentRow> {
  return requestJson(componentPath(id), {
    method: 'PATCH',
    body: { enabled },
    parse: (value) => {
      const row = isRecord(value) ? parseComponentRow(value.component) : null
      if (!row) throw new Error('Invalid custom UI component response')
      return row
    },
  })
}

export function deleteComponent(id: string): Promise<{ trashedAs: string }> {
  return requestJson(componentPath(id), {
    method: 'DELETE',
    parse: (value) => ({ trashedAs: isRecord(value) ? text(value.trashedAs, 256) : '' }),
  })
}

export function listComponentInstances(signal?: AbortSignal): Promise<ComponentInstancesData> {
  return requestJson('/api/custom-ui/instances', { signal, parse: parseComponentInstances })
}

export function createComponentInstance(input: {
  componentId: string
  surface: ComponentSurface
  size?: OverviewCardSize
  scope?: ComponentInstanceScope
  title?: string
  props?: Record<string, unknown>
}) {
  return requestJson('/api/custom-ui/instances', {
    method: 'POST',
    body: input,
    parse: parseInstanceResponse,
  })
}

export function updateComponentInstance(
  instanceId: string,
  patch: {
    size?: OverviewCardSize
    scope?: ComponentInstanceScope | null
    title?: string
    props?: Record<string, unknown>
    enabled?: boolean
  },
) {
  return requestJson(`/api/custom-ui/instances/${encodeURIComponent(instanceId)}`, {
    method: 'PATCH',
    body: patch,
    parse: parseInstanceResponse,
  })
}

export function deleteComponentInstance(instanceId: string): Promise<{ revision: number }> {
  return requestJson(`/api/custom-ui/instances/${encodeURIComponent(instanceId)}`, {
    method: 'DELETE',
    parse: (value) => ({
      revision: isRecord(value) && typeof value.revision === 'number' ? value.revision : 0,
    }),
  })
}

export function reorderComponentInstances(input: {
  baseRevision: number
  surface: ComponentSurface
  order: string[]
}) {
  return requestJson('/api/custom-ui/instances', {
    method: 'PUT',
    body: input,
    parse: parseComponentInstances,
  })
}

export function listComponentErrors(id: string, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/errors`, { signal, parse: parseComponentErrors })
}

export function reportComponentError(id: string, report: ComponentReport): Promise<void> {
  // keepalive：卸载或刷新前的最后一条错误也要送达；不读响应体。
  return requestJson(`${componentPath(id)}/errors`, {
    method: 'POST',
    body: report,
    keepalive: true,
    parse: () => undefined,
  })
}

export function componentStorageGet(id: string, key: string, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/storage/${encodeURIComponent(key)}`, {
    signal,
    parse: (value): { value: unknown } => {
      if (!isRecord(value) || !Object.hasOwn(value, 'value'))
        throw new Error('Invalid custom UI storage response')
      return { value: value.value }
    },
  })
}

export function componentStorageSet(id: string, key: string, value: unknown) {
  return requestJson(`${componentPath(id)}/storage/${encodeURIComponent(key)}`, {
    method: 'PUT',
    body: { value },
    parse: () => undefined,
  })
}

export function componentStorageRemove(id: string, key: string) {
  return requestJson(`${componentPath(id)}/storage/${encodeURIComponent(key)}`, {
    method: 'DELETE',
    parse: () => undefined,
  })
}

export function componentFetch(id: string, request: ComponentFetchRequest, signal?: AbortSignal) {
  return requestJson(`${componentPath(id)}/fetch`, {
    method: 'POST',
    body: request,
    signal,
    // 代理自己有 10 秒总超时；这里留出余量，避免客户端先超时、把 504 误报成网络错误。
    timeout: 15_000,
    parse: parseComponentFetchResult,
  })
}

// —— 绑定会话（计划 §5.4）：工作室“生成”与“让代理修复”先建 / 复用这个组件的代理会话 ——

export type ComponentAgentSession = {
  sessionId: string
  created: boolean
  // 会话为空时放在提示词前的技能命令（'/skill:pisper-component'）。
  command: string
  // 新建时 Runtime 带回的会话摘要（只加字段，旧 Runtime 没有）：交给看板并入缓存。
  session: SessionSummary | null
}

const SESSION_ID = /^[A-Za-z0-9._:-]{1,128}$/
const SKILL_COMMAND = /^\/skill:[a-z0-9][a-z0-9-]{0,63}$/

export function parseComponentAgentSession(value: unknown): ComponentAgentSession {
  if (!isRecord(value) || typeof value.sessionId !== 'string' || !SESSION_ID.test(value.sessionId))
    throw new Error('Invalid custom UI session response')
  const command =
    typeof value.command === 'string' && SKILL_COMMAND.test(value.command) ? value.command : ''
  const session =
    isRecord(value.session) && value.session.id === value.sessionId
      ? // 会话摘要沿用聊天接口的既有约定（requestJson<SessionSummary>）：只核对 id 与绑定的会话一致。
        (value.session as SessionSummary)
      : null
  return { sessionId: value.sessionId, created: value.created === true, command, session }
}

export function requestComponentAgentSession(id: string): Promise<ComponentAgentSession> {
  return requestJson(`${componentPath(id)}/session`, {
    method: 'POST',
    body: {},
    parse: parseComponentAgentSession,
  })
}
