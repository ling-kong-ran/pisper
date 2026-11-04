// 内置技能 pisper-component 的内容（SKILL.md 与 pisper-component.d.ts），供工作室绑定会话
// 用 /skill:pisper-component 显式加载（计划 §5.3）。必须是 JS 字符串：runtime-bundle.mjs 打 SEA /
// 手机闭包时会丢掉非 JS 文件。启动时由 SkillsService 按内容哈希写到 <dataDir>/builtin-skills/。
//
// .d.ts 里 interface Ctx / UiKit 的成员必须与 shared/custom-ui-manifest.mjs 的 CTX_V1_KEYS /
// UI_KIT_V1_KEYS 一致，由 runtime/tests/custom-ui-sdk-contract.test.mjs 守住。
//
// 写法说明：doc 标签基于 String.raw（反斜杠原样保留），只把 \` 与 \$ 还原成 ` 与 $，
// 这样文中的代码示例可以照常写模板字符串。

/** @param {TemplateStringsArray} strings @param {unknown[]} values */
function doc(strings, ...values) {
  return String.raw(strings, ...values).replace(/\\([`$])/g, '$1')
}

export const PISPER_COMPONENT_DTS = doc`// pisper-component.d.ts — Pisper 原生组件 SDK v1（ctx）类型声明。
// 组件用 JSDoc 引用：/** @param {import('./pisper-component').ComponentProps} input */

export type Surface = 'overview-card' | 'session-panel' | 'feature-page' | 'floating'

export interface ComponentSize {
  /** 放置单位：总览 S = 1、M = 2；其他放置为 1 */
  units: number
  width: number
  height: number
  /** 宽度 < 300 时为 true：只显示最关键的数字或一行文字 */
  compact: boolean
}

export interface ComponentPlacement {
  surface: Surface
  instanceId: string
  /** 实例自带的卡片标题（宿主在卡片头绘制，组件一般不用再画） */
  title: string
  /** 在工作室预览舞台里渲染时为 true */
  preview: boolean
}

export interface SessionSummary {
  id: string
  title: string
  cwd: string
  status: 'attention' | 'running' | 'queued' | 'done' | 'idle'
  updatedAt: string
  usage?: { totalTokens?: number }
}

export interface SessionFilter {
  cwd?: string
  status?: SessionSummary['status']
  /** 默认 50，最多 100 */
  limit?: number
}

export interface ComponentSessions {
  /** React hook：返回只读会话摘要列表，变化时自动重渲染（需要 sessions.read） */
  useList(filter?: SessionFilter): SessionSummary[]
  /** 订阅会话变化，返回取消订阅函数；卸载时自动取消（需要 sessions.read） */
  subscribe(listener: (sessions: SessionSummary[]) => void): () => void
  /** 在应用里打开某个会话（需要 sessions.open） */
  open(id: string): void
}

export interface FetchInit {
  method?: 'GET' | 'HEAD' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  /** 只允许 accept、accept-language、content-type、if-none-match、if-modified-since、authorization */
  headers?: Record<string, string>
  body?: string | ArrayBuffer | Uint8Array
  signal?: AbortSignal
}

export interface FetchResponse {
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

export interface ComponentNet {
  /** 经 Pisper Runtime 代理（需要 network，且主机在 hosts 里，或源在 privateOrigins 里） */
  fetch(url: string, init?: FetchInit): Promise<FetchResponse>
}

export interface ComponentStorage {
  /** 不存在时返回 undefined（需要 storage） */
  get<T = unknown>(key: string): Promise<T | undefined>
  set(key: string, value: unknown): Promise<void>
  remove(key: string): Promise<void>
}

export interface ProviderRequestInit extends FetchInit {
  /** origin 根路径，例如 /v1/usage；允许查询参数，路径与方法须在 manifest 声明 */
  path: string
  /** provider 沿用协议默认与已解析配置头；可显式指定同一凭据的认证头格式 */
  auth?: 'provider' | 'bearer' | 'x-api-key' | 'x-goog-api-key'
}

export interface ComponentProvider {
  request(input: ProviderRequestInit): Promise<FetchResponse>
}

export interface ComponentLog {
  info(...args: unknown[]): void
  warn(...args: unknown[]): void
  error(...args: unknown[]): void
}

export interface ThemeInfo {
  mode: 'light' | 'dark'
  density: string
  fontSize: string
  radius: string
  /** 读取 CSS 变量当前值，例如 token('--st-run') */
  token(name: string): string
}

export interface ComponentTheme {
  /** React hook */
  useTheme(): ThemeInfo
}

export type UiComponent = (props: Record<string, unknown>) => unknown

export interface UiKit {
  Button: UiComponent
  Badge: UiComponent
  Chip: UiComponent
  Input: UiComponent
  Textarea: UiComponent
  Switch: UiComponent
  Checkbox: UiComponent
  SegmentedControl: UiComponent
  Progress: UiComponent
  Spinner: UiComponent
  Skeleton: UiComponent
  Separator: UiComponent
  Kbd: UiComponent
  Tooltip: UiComponent
  TooltipTrigger: UiComponent
  TooltipContent: UiComponent
  TooltipProvider: UiComponent
  StatusGlyph: UiComponent
  StatusTag: UiComponent
  Empty: UiComponent
}

export declare class CapabilityError extends Error {
  capability:
    | 'network'
    | 'notify'
    | 'sessions.read'
    | 'sessions.open'
    | 'storage'
    | 'provider.request'
  reason: 'undeclared' | 'revoked'
}

export interface Ctx {
  sdk: 1
  /** 应用自己的 React 实例（组件里可以直接用 hooks） */
  React: typeof import('react')
  /** htm 标签模板，已绑定 React.createElement */
  html: (strings: TemplateStringsArray, ...values: unknown[]) => unknown
  ui: Readonly<UiKit>
  theme: ComponentTheme
  sessions: ComponentSessions
  /** 宿主的提示条，≤200 字，每分钟 ≤6 次（需要 notify） */
  notify(text: string): void
  net: ComponentNet
  storage: ComponentStorage
  provider: ComponentProvider
  /** 卸载或热重载时 abort */
  signal: AbortSignal
  setTimeout(handler: () => void, ms?: number): number
  setInterval(handler: () => void, ms?: number): number
  clearTimeout(id: number | null | undefined): void
  clearInterval(id: number | null | undefined): void
  size: ComponentSize
  placement: ComponentPlacement
  /** 宿主有当前会话时提供；工作室预览等场景可能为空 */
  session?: { id: string; title: string; cwd: string; status: SessionSummary['status'] }
  /** 读 manifest.i18n.<locale>.strings，{name} 占位符用 vars 替换 */
  t(key: string, vars?: Record<string, string | number>): string
  log: ComponentLog
  CapabilityError: typeof CapabilityError
}

export interface ComponentProps<P = Record<string, unknown>> {
  props: P
  size: ComponentSize
  placement: ComponentPlacement
  ctx: Ctx
}
`

export const PISPER_COMPONENT_SKILL_MD = doc`---
name: pisper-component
description: 编写与修改 Pisper 原生 UI 组件（manifest v2 + component.js + ctx SDK v1），用 component_create 写入。只在组件工作室的生成会话里用 /skill:pisper-component 显式加载。
disable-model-invocation: true
---

# Pisper 原生组件

你在为 Pisper 应用编写一个**原生组件**：一个浏览器可以直接执行的 ES 模块，由应用在自己的页面里渲染。
所有写入都通过 \`component_create\` 工具完成（不要用 write / edit 去改数据目录），工作室会自动热重载。

## 1. 原生组件是什么

- 组件与应用**同源运行，不是沙箱**：它和应用页面共享 React、样式与网络环境。
- manifest 里的 \`capabilities\` 是**透明清单**：\`ctx\` 只向组件提供声明过的能力，用户在工作室里能看到清单。它不是安全边界——所以只声明真正用到的能力，并如实写 \`reason\`。
- 组件不能直接执行 Node 代码，也不能读写用户文件。

## 2. 目录结构

~~~
<数据目录>/custom-ui/<id>/
  manifest.json      必需，见第 3 节
  component.js       入口（manifest.entry，默认 component.js）
  lib/*.js           可选：被入口相对导入的模块
  styles.css         可选：在 manifest.styles 里声明后由宿主注入
  *.json / *.svg     可选：数据与图标
~~~

- 只能写 \`.js .mjs .css .json .svg\`；不能有 \`.html\`；每个文件 ≤256 KiB，一次 ≤32 个、合计 ≤1 MiB。
- 组件目录最多 4 层、128 个文件、16 MiB。点开头的文件不会被服务。
- \`id\` 就是目录名：小写字母、数字、\`. _ -\`，≤64；\`pisper-island\` 是保留 id。

## 3. manifest v2

~~~json
{
  "runtime": "native",
  "name": "CI 状态",
  "version": "1.0.0",
  "description": "显示仓库最近一次 CI 运行",
  "entry": "component.js",
  "styles": [],
  "icon": "activity",
  "i18n": {
    "zh-CN": { "name": "CI 状态", "strings": { "loading": "加载中…", "empty": "还没有运行记录" } },
    "en-US": { "name": "CI status", "strings": { "loading": "Loading…", "empty": "No runs yet" } }
  },
  "props": {
    "type": "object",
    "properties": {
      "repo": { "type": "string", "title": "仓库", "description": "owner/name", "maxLength": 200 },
      "token": { "type": "string", "title": "访问令牌", "format": "text" },
      "limit": { "type": "integer", "title": "条数", "minimum": 1, "maximum": 10, "default": 3 }
    },
    "required": ["repo"]
  },
  "placements": [
    { "surface": "overview-card", "sizes": ["S", "M"] },
    { "surface": "session-panel" },
    { "surface": "feature-page" },
    { "surface": "floating", "size": { "w": 320, "h": 200 } }
  ],
  "capabilities": {
    "network": { "hosts": ["api.github.com"], "reason": "读取 GitHub Actions 运行记录" },
    "storage": { "quotaKb": 64, "reason": "缓存上次结果" },
    "notify": { "reason": "CI 失败时提醒" },
    "sessions.read": { "reason": "统计今天的会话" },
    "sessions.open": { "reason": "点击打开会话" }
  },
  "sdk": 1
}
~~~

字段规则：

- \`runtime\` 必须是 \`"native"\`；没有它的目录被当成旧版 iframe 组件（已不再支持）。
- \`name\` 1–120 字；\`version\` ≤64，缺省 \`0.0.0\`；\`description\` ≤500。
- \`entry\` 是 \`.js\` / \`.mjs\` 的相对路径；\`styles\` ≤4 个 \`.css\`。
- \`icon\` 是 lucide 图标名（\`^[a-z0-9-]+$\`，≤48）。
- \`i18n\` 只认 \`zh-CN\`、\`en-US\`：\`name\`、\`description\`、\`strings\`（≤200 键，每值 ≤500）。
- \`placements\` 缺省时四种放置都允许；\`overview-card\` 的 \`sizes\` 取 \`"S"\` / \`"M"\`；\`floating.size\` 的 w 160–480、h 64–480。
- \`capabilities\` 也可以写成数组：\`["notify", {"type": "network", "hosts": [...], "reason": "..."}]\`。
  \`network.hosts\` ≤16 个小写主机名，不能是 IP；\`*.example.com\` 只匹配恰好多一级的子域。
  \`network.privateOrigins\` ≤16 个局域网服务的源，写成规范的 \`http(s)://主机[:端口]\`（不带路径、末尾不加 \`/\`，默认端口不写），
  例如 \`"http://192.168.1.10:8080"\`、\`"https://nas.local"\`、\`"http://localhost:3000"\`；主机可以是 localhost、局域网名、私网或回环 IP。
  只访问私网服务时可以不写 \`hosts\`。
  \`storage.quotaKb\` 1–1024，缺省 256。
- \`sdk\` 缺省 1；\`minHostVersion\` 可写最低应用版本。

## 4. component.js 契约

~~~js
// @ts-check
/** @param {import('./pisper-component').ComponentProps<{ repo: string, limit?: number }>} input */
export default function CiStatus({ props, size, placement, ctx }) {
  const { React, html, ui, t } = ctx
  const [state, setState] = React.useState({ status: 'loading', runs: [] })

  React.useEffect(() => {
    if (!props.repo) return setState({ status: 'empty', runs: [] })
    ctx.net
      .fetch(\`https://api.github.com/repos/\${props.repo}/actions/runs?per_page=\${props.limit ?? 3}\`, {
        headers: { accept: 'application/vnd.github+json' },
        signal: ctx.signal,
      })
      .then((response) => (response.ok ? response.json() : Promise.reject(new Error(String(response.status)))))
      .then((data) => setState({ status: data.workflow_runs.length ? 'ready' : 'empty', runs: data.workflow_runs }))
      .catch((error) => {
        if (!ctx.signal.aborted) setState({ status: 'error', runs: [], message: String(error.message || error) })
      })
  }, [props.repo, props.limit])

  if (state.status === 'loading') return html\`<\${ui.Skeleton} style=\${{ height: '100%' }} />\`
  if (state.status === 'error') return html\`<div role="alert">\${state.message}</div>\`
  if (state.status === 'empty') return html\`<\${ui.Empty} title=\${t('empty')} />\`
  return html\`<ul style=\${{ margin: 0, padding: '10px 12px', listStyle: 'none' }}>
    \${state.runs.slice(0, size.compact ? 1 : 3).map(
      (run) => html\`<li key=\${run.id}>\${run.name} · \${run.conclusion || run.status}</li>\`,
    )}
  </ul>\`
}
~~~

- 默认导出一个函数组件 \`Component({ props, size, placement, ctx })\`。宿主把它当 React 组件渲染，可以用 hooks。
- **禁止裸模块导入**（\`import x from 'react'\`、\`import('lodash')\`、\`https://…\` 远程模块）：写入时会被拒绝。React、ui、html 都从 \`ctx\` 取。
- 相对导入可以用：\`import { chart } from './lib/chart.js'\`。
- 用 \`ctx.html\` 标签模板写界面（htm 语法：组件写成 \`<\${ui.Button}>\`，属性用 \`attr=\${value}\`，结束标签可以写 \`<//>\`）；也可以直接用 \`ctx.React.createElement\`。
- 没有 JSX / TypeScript 编译：直接写浏览器能跑的 JS，需要类型时用 JSDoc + 第 12 节的 .d.ts。

## 5. ctx SDK v1

| 成员 | 说明 | 需要的能力 |
| --- | --- | --- |
| \`sdk\` | 恒为 1 | — |
| \`React\` | 应用的 React 实例 | — |
| \`html\` | htm 标签模板（绑定 React.createElement） | — |
| \`ui\` | 只读 UI 子集：Button、Badge、Chip、Input、Textarea、Switch、Checkbox、SegmentedControl、Progress、Spinner、Skeleton、Separator、Kbd、Tooltip（含 Trigger / Content / Provider）、StatusGlyph、StatusTag、Empty | — |
| \`theme.useTheme()\` | \`{ mode, density, fontSize, radius, token(name) }\` | — |
| \`sessions.useList(filter?)\` | 只读会话摘要 \`{id, title, cwd, status, updatedAt, usage?}[]\`；filter \`{cwd?, status?, limit≤100}\` | sessions.read |
| \`sessions.subscribe(fn)\` | 会话变化订阅，返回取消函数 | sessions.read |
| \`sessions.open(id)\` | 在应用里打开会话 | sessions.open |
| \`notify(text)\` | 应用提示条，≤200 字，每分钟 ≤6 次 | notify |
| \`net.fetch(url, init)\` | 经 Runtime 代理（不受 CORS 限制）；目标支持 CORS 时也可以直接 \`fetch\`，见第 8 节 | network |
| \`storage.get/set/remove(key)\` | 组件私有存储，key \`^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,127}$\`，值是 JSON | storage |
| \`provider.request()\` | 使用当前会话的 Provider 认证发出请求，返回上游原始 Response（由组件自行解析）；原始 Key 不进入组件 | provider.request |
| \`signal\` | 卸载或热重载时 abort：传给 fetch、在回调里检查 | — |
| \`setTimeout\` / \`setInterval\` / \`clearTimeout\` / \`clearInterval\` | 托管定时器，卸载时自动清理 | — |
| \`size\` | \`{ units, width, height, compact }\`，随容器实时变化 | — |
| \`placement\` | \`{ surface, instanceId, title, preview }\` | — |
| \`session\` | 宿主有当前会话时提供 \`{ id, title, cwd, status }\`；工作室预览等场景可能为空 | — |
| \`t(key, vars)\` | 读 manifest.i18n 的 strings，按应用语言选，\`{name}\` 占位 | — |
| \`log.info/warn/error\` | 写入工作室日志页；warn / error 也会上报 | — |
| \`CapabilityError\` | 调用未声明、或被用户撤销的能力时抛出的错误类（\`reason\` 为 \`'undeclared'\` 或 \`'revoked'\`） | — |

调用未声明的能力、或用户在工作室能力清单里撤销了的能力，会抛 \`CapabilityError\` 并记一条日志；组件应当捕获它并降级显示（例如“网络已被撤销”），不要整块崩溃。定时器、订阅与 net 的回调里抛出的异常会归到这个组件，60 秒内崩溃 3 次的实例会被暂停。

认证请求应声明 \`provider.request: { paths: ["/v1/usage"], methods: ["GET"], reason: "读取额度" }\`，调用 \`ctx.provider.request({ path: "/v1/usage", auth: "bearer" })\`。不在 props 中增加 API Key，不自行拼接鉴权头。Runtime 只负责认证和传输，各家额度路径、响应解析由组件实现。没有当前会话会拒绝请求。

路径从当前模型 baseUrl 的 origin 根目录解析（不是拼接 baseUrl 的路径）；仅支持 HTTPS 443 公网服务。manifest 中 paths 是精确 pathname（≤16 个），methods 是 GET/HEAD/POST/PUT/PATCH/DELETE 的明确列表；不支持通配、路径编码、点段、绝对 URL 或跨域重定向。查询参数可以随请求提供，不参与路径匹配。认证的动态 baseUrl 若改变 origin 返回 origin_denied，不猜测其他域名。特殊签名或无法解析的认证返回 auth_unsupported。可显式选择 bearer、x-api-key、x-goog-api-key 格式；默认 provider 采用协议认证和配置头。

网络请求的 DNS 固定、私网阻断、响应白名单、2 MiB 响应限制、每组件 30 次/分钟和并发 4 次同样适用；认证请求全流程 10 秒超时，卸载或组件 signal 取消时中止。若系统 DNS 全部返回 TUN 的 198.18/15 合成地址，仅向 Cloudflare DoH 发送目标域名重新获取公网地址，不携带认证，仍拒绝任何受限地址。返回包含 ok/status/headers/text/json 等，不提供原始 Key、环境变量或认证头。上游缺失额度窗口不能解释为无限额度。

## 6. props schema 子集

\`manifest.props\` 是 JSON Schema 的子集，顶层 \`{ "type": "object", "properties": {…}, "required"?: […] }\`，≤32 个属性：

- \`string\`：\`format\` 取 \`text\` / \`textarea\` / \`url\` / \`color\`，\`maxLength\` ≤4000；
- \`number\` / \`integer\`：\`minimum\`、\`maximum\`、\`step\`、\`x-unit\`；
- \`boolean\`：开关；
- 枚举：\`{ "type": "string", "enum": [...] }\`（≤32 项，≤4 项显示为分段控件）；
- \`array\` + \`items: { "type": "string" }\`（≤50 项）：标签输入。

每个字段可带 \`title\`、\`description\`、\`default\`、\`x-i18n\`（按语言的 title / description）。
\`$ref\`、\`oneOf\`、\`pattern\`、嵌套对象等关键字不支持，会被报错拒绝。属性按放置实例保存在本机数据目录（明文），保存时 Runtime 会再校验一遍；组件拿到的 \`props\` 只含用户填过的值，自己处理缺省。

## 7. 放置与尺寸

| 放置 | 尺寸 | 说明 |
| --- | --- | --- |
| \`overview-card\` | S 320×120、M 320×248（手机上全宽） | 全部代理页右侧组件栏，可多实例 |
| \`session-panel\` | 宽 456 或 420，高随面板 | 会话检查器里的标签，每个组件 1 个 |
| \`feature-page\` | 整页 | 功能 › 我的组件 |
| \`floating\` | manifest 声明的 \`{w, h}\`（缺省 320×200） | 桌面浮窗，手机上不显示 |

- 卡片头（标题、菜单）由宿主绘制，组件只画内容区；容器 \`overflow: hidden\`。
- 内容区**没有内边距**，组件自己留白：根元素建议 \`padding: '10px 12px'\`（与宿主卡片头的 12px 左缩进对齐），否则文字会贴着卡片边。
- **必须能在最小尺寸下渲染**：S 只有 120 高；\`size.compact\`（宽 < 300）时只显示最关键的信息。
- 不要假设固定像素，跟着 \`ctx.size\` 布局。

## 8. 网络规则

组件有两种联网方式，按目标选：

**直接 \`fetch\` / \`XMLHttpRequest\` / \`WebSocket\`**：组件与应用同源运行，可以直接请求。适合目标服务允许跨域（返回 CORS 头）的情况，
本机或局域网服务也可以。浏览器的限制照常生效：目标没有 CORS 头会被拦；应用以 https 打开时（远程访问、部分桌面平台）请求 \`http://\` 会被当成混合内容拦掉。
直连不经过代理，不计入工作室的能力清单，也不受下面的限流；不要带上应用的 Cookie（\`credentials\` 保持默认），不要请求应用自己的 \`/api/\`。

**\`ctx.net.fetch\`（经 Runtime 代理）**：目标没有 CORS 头、或遇到 https 页面请求 http 的限制时用它；请求从 Runtime 发出，不受浏览器跨域限制：

- 公网只允许 \`https://\`、443 端口、\`hosts\` 里声明过的主机；不能用 IP 地址；
- 公网主机在 DNS 解析后拒绝回环、私网、链路本地与元数据地址，每次重定向都重新校验（≤5 跳）；
- 本机或局域网 / 内网接口（本机服务、NAS、路由器、Home Assistant、公司内网服务等）在 \`network.privateOrigins\` 里逐个声明源：
  请求的协议、主机和端口与某一项完全一致时，允许 \`http://\`、任意端口和私网 IP；解析结果必须全部是私网地址
  或本机回环地址（10/8、172.16/12、192.168/16、100.64/10、127/8、fc00::/7、::1），链路本地与元数据地址仍然拒绝；
  这类请求到不了 Pisper 自己的端口（会被 403），只能访问你自己的服务；
  https 仍校验证书，自签名证书的服务改用 http 源；
- 10 秒超时，响应 ≤2 MB，请求体 ≤384 KiB；每个组件每分钟 30 次、并发 ≤4；
- 请求头只允许 \`accept\`、\`accept-language\`、\`content-type\`、\`if-none-match\`、\`if-modified-since\`、\`authorization\`（只能是组件自己的值，例如用户在属性里填的令牌）；
- 绝不带应用的 Cookie 或凭据；不要在源码里写死任何密钥。

## 9. 样式

- 优先用 \`ctx.ui\` 组件，以及 \`style={{ … }}\` 里的 CSS 变量：\`var(--text)\`、\`var(--text-secondary)\`、\`var(--text-muted)\`、\`var(--stroke)\`、\`var(--surface-subtle)\`、\`var(--surface-active)\`、\`var(--st-run)\`、\`var(--st-attn)\`、\`var(--st-fail)\` 等；它们自动跟随明暗主题。
- 不要写 \`var(--color-…)\`：那是应用 CSS 编译期的别名，运行时不存在，写了只会退回继承的颜色。
- **组件源码里写的 Tailwind 类不会被编译**，只有应用 CSS 里恰好存在的类才有效——不要依赖它。
- 需要更多样式就写自己的 \`.css\` 并在 \`manifest.styles\` 声明；选择器加上组件前缀（例如 \`.ci-status-row\`），因为 CSS 作用于整个页面。
- 琥珀色（attn）只表示“需要你处理”，不要拿来做装饰。

## 10. 用 component_create 新建或更新

- 新建：\`{ "mode": "create", "id": "ci-status", "manifest": {…}, "files": [{ "path": "component.js", "content": "…" }] }\`。
- 更新：\`{ "mode": "update", "id": "ci-status", "files": [只传改动的文件], "deleteFiles": [...], "summary": "一句话说明" }\`；不传 manifest 就保留原来的。
- 每次更新前 Runtime 自动保存上一个版本的快照（最多 20 份）。
- 校验失败会返回结构化错误清单（\`path\` + \`code\`），逐项修正后重试，不要换个 id 绕过。
- 写入后工作室和所有已放置的实例自动热重载。新组件不会自动放到任何位置，由用户在工作室里选择放置。

## 11. 自检清单

- [ ] 加载中、空数据、错误、未配置（缺少必填属性）四种状态都有界面，不会白屏或抛错；
- [ ] S（320×120）与 compact 下内容不溢出、不被裁成半行；
- [ ] 定时器用 \`ctx.setTimeout\` / \`ctx.setInterval\`，请求传 \`ctx.signal\`，回调里检查 \`ctx.signal.aborted\`；
- [ ] 交互元素可以用键盘到达，按钮有可读名称，状态文字用 \`role="status"\` 或 \`role="alert"\`；
- [ ] 文案放进 \`manifest.i18n.*.strings\`，中英两份都写；
- [ ] 只声明真正用到的能力；不写死密钥；没有裸导入。

## 12. pisper-component.d.ts

~~~ts
${PISPER_COMPONENT_DTS}~~~
`

/** 内置技能清单：SkillsService 启动时按内容哈希写到 <dataDir>/builtin-skills/<name>/。 */
export const BUILTIN_SKILLS = Object.freeze([
  Object.freeze({
    name: 'pisper-component',
    files: Object.freeze({
      'SKILL.md': PISPER_COMPONENT_SKILL_MD,
      'pisper-component.d.ts': PISPER_COMPONENT_DTS,
    }),
  }),
])
