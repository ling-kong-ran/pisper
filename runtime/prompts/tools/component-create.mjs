// component_create 的说明（非工作室会话里的精简契约，完整说明在内置技能 pisper-component）。

export const COMPONENT_CREATE = Object.freeze({
  description:
    'Create or update a Pisper native UI component (an ES module rendered same-origin inside the Pisper app).',
  toolDescription:
    'Create or update a Pisper native UI component (an ES module rendered same-origin inside the Pisper app). Writes <data dir>/custom-ui/<id>/ and follows the session permission mode like an ordinary file write.',
  snippet: 'Create or update a Pisper native UI component with component_create',
  // 非工作室会话里的精简契约（完整说明在内置技能 pisper-component，用 /skill:pisper-component 加载）。
  guidelines: [
    'Use component_create when the user asks for a Pisper component, widget or card. For the full SDK reference load /skill:pisper-component.',
    'manifest: {"runtime":"native","name":…,"version":"1.0.0","entry":"component.js","placements":[{"surface":"overview-card","sizes":["S","M"]}],"props":{"type":"object","properties":{…}}}. Full placement and capability schemas are in /skill:pisper-component.',
    'component.js: export default function Component({ props, size, placement, ctx }) { const { html } = ctx; return html`<div>…</div>` }. No bare imports (React, ui and html come from ctx); relative imports like ./lib.js are fine. No .html files.',
    'Network: fetch() directly if the target allows CORS, else ctx.net.fetch (https to declared hosts; local/LAN via network.privateOrigins e.g. "http://localhost:3000"). Declare capabilities for ctx.net/storage/notify/sessions. Use ctx.setTimeout/setInterval and ctx.signal so timers stop on unmount.',
    'For saved auth declare provider.request with exact paths/methods; call ctx.provider.request({path}). Runtime keeps credentials private. Never add API Key props. Parse provider data in the component.',
    'Sizes: overview S 320×120, M 320×248; ctx.size.compact is true under 300 px wide; render loading, empty, error and disabled states. Style with ctx.ui and CSS variables (var(--…)); Tailwind classes in component source are not compiled.',
    'mode "update" sends only changed files (plus deleteFiles); omit manifest to keep it. Each update snapshots the previous version. New components are not placed anywhere until the user places them in the component studio.',
  ],
  params: {
    path: 'Component-relative path (.js .mjs .css .json .svg), e.g. component.js or lib/chart.js; not manifest.json',
    content: 'UTF-8 file content',
    mode: 'create a new component directory, or update an existing native component',
    id: 'Component id = directory name (lowercase letters, digits, . _ -)',
    manifest:
      'manifest.json object (runtime must be "native"); required for create, optional for update',
    files: 'Files to write (≤32, ≤256 KiB each, ≤1 MiB total)',
    deleteFiles: 'Component-relative paths to delete (update only)',
    summary: 'One-line change summary for the version list',
  },
})
