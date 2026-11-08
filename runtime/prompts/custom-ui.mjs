// 内置技能 pisper-component 的正文（SKILL.md 与 pisper-component.d.ts），供工作室绑定会话
// 用 /skill:pisper-component 显式加载（计划 §5.3）。必须是 JS 字符串：runtime-bundle.mjs 打 SEA /
// 手机闭包时会丢掉非 JS 文件。技能清单与写盘在 runtime/services/custom-ui-sdk-docs.mjs。
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

export const PISPER_COMPONENT_DTS = doc`// pisper-component.d.ts — Pisper native component SDK v1 (ctx) types.
// Reference from JSDoc: /** @param {import('./pisper-component').ComponentProps} input */

export type Surface = 'overview-card' | 'session-panel' | 'feature-page' | 'floating'

export interface ComponentSize {
  /** Placement units: overview S = 1, M = 2; other placements 1 */
  units: number
  width: number
  height: number
  /** true when width < 300: show only the key number or one line */
  compact: boolean
}

export interface ComponentPlacement {
  surface: Surface
  instanceId: string
  /** Instance card title (drawn by the host in the card header) */
  title: string
  /** true inside the studio preview stage */
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
  /** default 50, max 100 */
  limit?: number
}

export interface ComponentSessions {
  /** React hook: read-only session summaries, re-renders on change (sessions.read) */
  useList(filter?: SessionFilter): SessionSummary[]
  /** Subscribe to session changes; returns unsubscribe, auto-cancelled on unmount (sessions.read) */
  subscribe(listener: (sessions: SessionSummary[]) => void): () => void
  /** Open a session in the app (sessions.open) */
  open(id: string): void
}

export interface FetchInit {
  method?: 'GET' | 'HEAD' | 'POST' | 'PUT' | 'PATCH' | 'DELETE'
  /** Only accept, accept-language, content-type, if-none-match, if-modified-since, authorization */
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
  /** Proxied by the Pisper Runtime (network; host in hosts or origin in privateOrigins) */
  fetch(url: string, init?: FetchInit): Promise<FetchResponse>
}

export interface ComponentStorage {
  /** undefined when missing (storage) */
  get<T = unknown>(key: string): Promise<T | undefined>
  set(key: string, value: unknown): Promise<void>
  remove(key: string): Promise<void>
}

export interface ProviderRequestInit extends FetchInit {
  /** Path from the origin root, e.g. /v1/usage; query allowed; path and method must be declared */
  path: string
  /** provider = protocol default plus resolved config headers; or force a header format for the same credential */
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
  /** Current value of a CSS variable, e.g. token('--st-run') */
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
  /** The app's own React (hooks work) */
  React: typeof import('react')
  /** htm tagged template bound to React.createElement */
  html: (strings: TemplateStringsArray, ...values: unknown[]) => unknown
  ui: Readonly<UiKit>
  theme: ComponentTheme
  sessions: ComponentSessions
  /** Host toast, ≤200 chars, ≤6 per minute (notify) */
  notify(text: string): void
  net: ComponentNet
  storage: ComponentStorage
  provider: ComponentProvider
  /** Aborted on unmount or hot reload */
  signal: AbortSignal
  setTimeout(handler: () => void, ms?: number): number
  setInterval(handler: () => void, ms?: number): number
  clearTimeout(id: number | null | undefined): void
  clearInterval(id: number | null | undefined): void
  size: ComponentSize
  placement: ComponentPlacement
  /** Present when the host has a current session; may be missing, e.g. in studio preview */
  session?: { id: string; title: string; cwd: string; status: SessionSummary['status'] }
  /** Reads manifest.i18n.<locale>.strings; {name} placeholders are filled from vars */
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
description: Write or edit Pisper native UI components (manifest v2 + component.js + ctx SDK v1), saved with component_create. Loaded explicitly with /skill:pisper-component in the component studio session only.
disable-model-invocation: true
---

# Pisper native components

A native component is an ES module the browser runs directly inside the app's page. Write everything with \`component_create\` (never write / edit the data directory); the studio hot-reloads.

## 1. What it is

- Same origin as the app, **not sandboxed**: it shares React, styles and network with the app page.
- \`capabilities\` in the manifest is a **transparent list**: \`ctx\` exposes only declared capabilities and the user sees the list in the studio. It is not a security boundary, so declare only what you use, with an honest \`reason\`.
- No Node code, no access to user files.

## 2. Layout

~~~
<data dir>/custom-ui/<id>/
  manifest.json      required, see §3
  component.js       entry (manifest.entry, default component.js)
  lib/*.js           optional modules imported relatively from the entry
  styles.css         optional, injected when listed in manifest.styles
  *.json / *.svg     optional data and icons
~~~

- Only \`.js .mjs .css .json .svg\`, no \`.html\`; each file ≤256 KiB, ≤32 files and ≤1 MiB per write.
- Folder: ≤4 levels, 128 files, 16 MiB. Dotfiles are not served.
- \`id\` is the folder name: lowercase letters, digits, \`. _ -\`, ≤64; \`pisper-island\` is reserved.

## 3. manifest v2

~~~json
{
  "runtime": "native",
  "name": "CI status",
  "version": "1.0.0",
  "description": "Latest CI run of a repository",
  "entry": "component.js",
  "styles": [],
  "icon": "activity",
  "i18n": {
    "en-US": { "name": "CI status", "strings": { "loading": "Loading…", "empty": "No runs yet" } },
    "zh-CN": { "name": "…", "strings": { "loading": "…", "empty": "…" } }
  },
  "props": {
    "type": "object",
    "properties": {
      "repo": { "type": "string", "title": "Repository", "description": "owner/name", "maxLength": 200 },
      "token": { "type": "string", "title": "Access token", "format": "text" },
      "limit": { "type": "integer", "title": "Count", "minimum": 1, "maximum": 10, "default": 3 }
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
    "network": { "hosts": ["api.github.com"], "reason": "Read GitHub Actions runs" },
    "storage": { "quotaKb": 64, "reason": "Cache the last result" },
    "notify": { "reason": "Alert when CI fails" },
    "sessions.read": { "reason": "Count today's sessions" },
    "sessions.open": { "reason": "Open a session on click" }
  },
  "sdk": 1
}
~~~

- \`runtime\` must be \`"native"\`; folders without it are legacy iframe components (no longer supported).
- \`name\` 1–120 chars; \`version\` ≤64, default \`0.0.0\`; \`description\` ≤500.
- \`entry\` is a relative \`.js\` / \`.mjs\` path; \`styles\` ≤4 \`.css\` files.
- \`icon\` is a lucide icon name (\`^[a-z0-9-]+$\`, ≤48).
- \`i18n\` accepts only \`zh-CN\` and \`en-US\`: \`name\`, \`description\`, \`strings\` (≤200 keys, values ≤500).
- Without \`placements\` all four are allowed; \`overview-card\` \`sizes\` are \`"S"\` / \`"M"\`; \`floating.size\` w 160–480, h 64–480.
- \`capabilities\` may also be an array: \`["notify", {"type": "network", "hosts": [...], "reason": "..."}]\`.
  \`network.hosts\`: ≤16 lowercase host names, no IPs; \`*.example.com\` matches exactly one extra label.
  \`network.privateOrigins\`: ≤16 LAN service origins in canonical \`http(s)://host[:port]\` form (no path, no trailing \`/\`, no default port),
  e.g. \`"http://192.168.1.10:8080"\`, \`"https://nas.local"\`, \`"http://localhost:3000"\`; the host may be localhost, a LAN name, or a private or loopback IP.
  \`hosts\` may be omitted when only private services are used.
  \`storage.quotaKb\` 1–1024, default 256.
- \`sdk\` defaults to 1; \`minHostVersion\` sets a minimum app version.

## 4. component.js contract

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

- Default-export a function component \`Component({ props, size, placement, ctx })\`; the host renders it as a React component, hooks work.
- **No bare imports** (\`import x from 'react'\`, \`import('lodash')\`, \`https://…\` modules): rejected on write. Take React, ui and html from \`ctx\`.
- Relative imports are fine: \`import { chart } from './lib/chart.js'\`.
- Write UI with the \`ctx.html\` template (htm: components as \`<\${ui.Button}>\`, attributes \`attr=\${value}\`, \`<//>\` closes), or \`ctx.React.createElement\`.
- No JSX / TypeScript build: plain browser JS; use JSDoc + the .d.ts in §12 for types.

## 5. ctx SDK v1

| Member | Notes | Capability |
| --- | --- | --- |
| \`sdk\` | always 1 | — |
| \`React\` | the app's React | — |
| \`html\` | htm template bound to React.createElement | — |
| \`ui\` | read-only UI subset: Button, Badge, Chip, Input, Textarea, Switch, Checkbox, SegmentedControl, Progress, Spinner, Skeleton, Separator, Kbd, Tooltip (+ Trigger / Content / Provider), StatusGlyph, StatusTag, Empty | — |
| \`theme.useTheme()\` | \`{ mode, density, fontSize, radius, token(name) }\` | — |
| \`sessions.useList(filter?)\` | read-only summaries \`{id, title, cwd, status, updatedAt, usage?}[]\`; filter \`{cwd?, status?, limit≤100}\` | sessions.read |
| \`sessions.subscribe(fn)\` | session change subscription, returns unsubscribe | sessions.read |
| \`sessions.open(id)\` | open a session in the app | sessions.open |
| \`notify(text)\` | app toast, ≤200 chars, ≤6 per minute | notify |
| \`net.fetch(url, init)\` | via the Runtime proxy (no CORS limits); plain \`fetch\` also works when the target sends CORS headers, see §8 | network |
| \`storage.get/set/remove(key)\` | private storage, key \`^[a-zA-Z0-9][a-zA-Z0-9._:-]{0,127}$\`, JSON values | storage |
| \`provider.request()\` | request with the current session's provider auth; returns the raw upstream response for the component to parse; the key never reaches the component | provider.request |
| \`signal\` | aborted on unmount / hot reload: pass to fetch, check in callbacks | — |
| \`setTimeout\` / \`setInterval\` / \`clearTimeout\` / \`clearInterval\` | managed timers, cleared on unmount | — |
| \`size\` | \`{ units, width, height, compact }\`, live | — |
| \`placement\` | \`{ surface, instanceId, title, preview }\` | — |
| \`session\` | \`{ id, title, cwd, status }\` when the host has a current session; may be missing (e.g. studio preview) | — |
| \`t(key, vars)\` | manifest.i18n strings in the app language, \`{name}\` placeholders | — |
| \`log.info/warn/error\` | studio log page; warn / error are also reported | — |
| \`CapabilityError\` | thrown for undeclared or user-revoked capabilities (\`reason\` \`'undeclared'\` or \`'revoked'\`) | — |

Catch \`CapabilityError\` and degrade (e.g. "network revoked") instead of crashing. Exceptions in timers, subscriptions and net callbacks count against the component; an instance that crashes 3 times in 60 s is paused.

Authenticated requests: declare \`provider.request: { paths: ["/v1/usage"], methods: ["GET"], reason: "Read quota" }\` and call \`ctx.provider.request({ path: "/v1/usage", auth: "bearer" })\`. Don't add API key props or build auth headers yourself. The Runtime only authenticates and transports; quota paths and parsing are up to the component. Rejected without a current session.

- The path resolves from the origin root of the current model's baseUrl (not appended to its path); HTTPS on 443 to public hosts only.
- \`paths\` are exact pathnames (≤16); \`methods\` an explicit list of GET/HEAD/POST/PUT/PATCH/DELETE. No wildcards, encoded paths, dot segments, absolute URLs or cross-origin redirects. Query strings are allowed and not matched.
- A dynamic baseUrl that changes origin returns \`origin_denied\`; special signing or unresolvable auth returns \`auth_unsupported\`. \`auth\` may force bearer, x-api-key or x-goog-api-key; default \`provider\` uses protocol auth and config headers.
- Network rules from §8 apply (DNS pinning, private blocking, response allowlist, 2 MiB, 30/min, 4 concurrent); 10 s total timeout; aborted on unmount or \`ctx.signal\`. If system DNS only returns TUN 198.18/15 addresses, only the host name is re-resolved via Cloudflare DoH, without auth, and restricted addresses are still refused.
- The response has ok/status/headers/text/json etc.; never the key, env vars or auth headers. A missing quota window upstream does not mean unlimited.

## 6. props schema subset

\`manifest.props\` is a JSON Schema subset: top level \`{ "type": "object", "properties": {…}, "required"?: […] }\`, ≤32 properties:

- \`string\`: \`format\` \`text\` / \`textarea\` / \`url\` / \`color\`, \`maxLength\` ≤4000;
- \`number\` / \`integer\`: \`minimum\`, \`maximum\`, \`step\`, \`x-unit\`;
- \`boolean\`: switch;
- enum: \`{ "type": "string", "enum": [...] }\` (≤32 items; ≤4 render as a segmented control);
- \`array\` + \`items: { "type": "string" }\` (≤50): tag input.

Fields may have \`title\`, \`description\`, \`default\`, \`x-i18n\` (per-locale title / description).
\`$ref\`, \`oneOf\`, \`pattern\`, nested objects etc. are rejected. Props are stored per placement instance in the local data folder (plain text) and re-validated on save; \`props\` holds only values the user filled in, so handle defaults yourself.

## 7. Placements and sizes

| Placement | Size | Notes |
| --- | --- | --- |
| \`overview-card\` | S 320×120, M 320×248 (full width on phones) | widget rail of the all-agents page, multiple instances |
| \`session-panel\` | width 456 or 420, height follows the panel | tab in the session inspector, one per component |
| \`feature-page\` | full page | Features › My components |
| \`floating\` | manifest \`{w, h}\` (default 320×200) | desktop floating window, hidden on phones |

- The host draws the card header (title, menu); the component draws only the content area, which has \`overflow: hidden\`.
- The content area has **no padding**: pad the root yourself, e.g. \`padding: '10px 12px'\` (matches the header's 12px inset).
- **Must render at the smallest size**: S is only 120 high; when \`size.compact\` (width < 300) show only the key information.
- Don't assume fixed pixels; lay out from \`ctx.size\`.

## 8. Network rules

Two ways to reach the network:

**Plain \`fetch\` / \`XMLHttpRequest\` / \`WebSocket\`**: the component runs same-origin, so direct requests work when the target sends CORS headers, including local or LAN services. Browser limits apply: no CORS headers means blocked; when the app is served over https (remote access, some desktop platforms) \`http://\` requests are blocked as mixed content. Direct requests skip the proxy, the capability list and the limits below. Keep \`credentials\` default (no app cookies) and never call the app's own \`/api/\`.

**\`ctx.net.fetch\` (Runtime proxy)**: for targets without CORS headers or http targets from an https page; requests leave from the Runtime:

- Public: only \`https://\`, port 443, hosts declared in \`hosts\`; no IP addresses.
- Public hosts are refused after DNS when they resolve to loopback, private, link-local or metadata addresses; every redirect is re-checked (≤5 hops).
- Local / LAN services (local servers, NAS, routers, Home Assistant, intranet) are declared one origin each in \`network.privateOrigins\`. When scheme, host and port match exactly, \`http://\`, any port and private IPs are allowed; every resolved address must be private or loopback (10/8, 172.16/12, 192.168/16, 100.64/10, 127/8, fc00::/7, ::1); link-local and metadata stay refused. Pisper's own port is refused (403). https still verifies certificates; use an http origin for self-signed services.
- 10 s timeout, response ≤2 MB, request body ≤384 KiB; per component 30 per minute, ≤4 concurrent.
- Request headers: only \`accept\`, \`accept-language\`, \`content-type\`, \`if-none-match\`, \`if-modified-since\`, \`authorization\` (the component's own value, e.g. a token from props).
- Never sends app cookies or credentials; never hard-code secrets.

## 9. Styles

- Prefer \`ctx.ui\` and CSS variables in \`style={{ … }}\`: \`var(--text)\`, \`var(--text-secondary)\`, \`var(--text-muted)\`, \`var(--stroke)\`, \`var(--surface-subtle)\`, \`var(--surface-active)\`, \`var(--st-run)\`, \`var(--st-attn)\`, \`var(--st-fail)\` etc.; they follow light / dark themes.
- Don't use \`var(--color-…)\`: those are compile-time aliases of the app CSS and don't exist at runtime.
- **Tailwind classes in component source are not compiled**; only classes that happen to exist in the app CSS work. Don't rely on them.
- For more styles, ship a \`.css\` listed in \`manifest.styles\`, with prefixed selectors (e.g. \`.ci-status-row\`) since CSS applies to the whole page.
- Amber (attn) means only "needs you"; not decoration.

## 10. component_create

- Create: \`{ "mode": "create", "id": "ci-status", "manifest": {…}, "files": [{ "path": "component.js", "content": "…" }] }\`.
- Update: \`{ "mode": "update", "id": "ci-status", "files": [changed files only], "deleteFiles": [...], "summary": "one line" }\`; omit manifest to keep it.
- Each update snapshots the previous version first (up to 20).
- Validation failures return a structured list (\`path\` + \`code\`); fix each and retry, don't switch ids to get around it.
- The studio and all placed instances hot-reload. New components aren't placed anywhere; the user places them in the studio.

## 11. Checklist

- [ ] Loading, empty, error and unconfigured (missing required props) states all render; no blank screen or throw.
- [ ] Nothing overflows or gets cut at S (320×120) and compact.
- [ ] Timers via \`ctx.setTimeout\` / \`ctx.setInterval\`; requests take \`ctx.signal\`; callbacks check \`ctx.signal.aborted\`.
- [ ] Interactive elements reachable by keyboard, buttons have accessible names, status text uses \`role="status"\` or \`role="alert"\`.
- [ ] Text lives in \`manifest.i18n.*.strings\`, both zh-CN and en-US.
- [ ] Only used capabilities declared; no hard-coded secrets; no bare imports.

## 12. pisper-component.d.ts

~~~ts
${PISPER_COMPONENT_DTS}~~~
`
