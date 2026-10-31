// 组件创建工具：代理新建或更新 Pisper 原生 UI 组件（<数据目录>/custom-ui/<id>/，规格 5.2、计划 §5.1）。
// 校验与写入都在 CustomUiService.writeComponent（模板创建共用同一份），这里只做参数 schema、
// 结构化错误包装与结果摘要。权限按普通写文件处理：跟随会话权限模式，不算 critical（负责人决定）。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'

export const manifest = {
  id: 'component_create',
  name: 'Component Create',
  category: 'plugins',
  risk: 'high',
  description:
    'Create or update a Pisper native UI component (an ES module rendered same-origin inside the Pisper app).',
  scope: 'Pisper data directory custom-ui/<id>',
  capability:
    'Write component manifest, JavaScript, CSS, JSON and SVG files with a version snapshot before each update; hot reloads open views',
  source: 'app',
}

// 非工作室会话里的精简契约（完整说明在内置技能 pisper-component，用 /skill:pisper-component 加载）。
const PROMPT_GUIDELINES = [
  'Use component_create when the user asks for a Pisper component, widget or card. For the full SDK reference load /skill:pisper-component.',
  'manifest: {"runtime":"native","name":…,"version":"1.0.0","entry":"component.js","placements":[{"surface":"overview-card","sizes":["S","M"]},{"surface":"session-panel"},{"surface":"feature-page"},{"surface":"floating","size":{"w":320,"h":200}}],"capabilities":{"network":{"hosts":["api.example.com"],"reason":"…"},"storage":{"quotaKb":64},"notify":true,"sessions.read":true},"props":{"type":"object","properties":{…}}}.',
  'component.js: export default function Component({ props, size, placement, ctx }) { const { html } = ctx; return html`<div>…</div>` }. No bare imports (React, ui and html come from ctx); relative imports like ./lib.js are fine. No .html files.',
  'Only declared capabilities work: ctx.net.fetch (https to declared hosts, 10 s, 2 MB, no app cookies), ctx.storage.get/set/remove, ctx.notify, ctx.sessions.useList/open. Use ctx.setTimeout/ctx.setInterval and ctx.signal so timers stop on unmount.',
  'Sizes: overview S 320×120, M 320×248; ctx.size.compact is true under 300 px wide; render loading, empty, error and disabled states. Style with ctx.ui and CSS variables (var(--…)); Tailwind classes in component source are not compiled.',
  'mode "update" sends only changed files (plus deleteFiles); omit manifest to keep it. Each update snapshots the previous version. New components are not placed anywhere until the user places them in the component studio.',
]

const fileSchema = Type.Object({
  path: Type.String({
    minLength: 1,
    maxLength: 200,
    description:
      'Component-relative path (.js .mjs .css .json .svg), e.g. component.js or lib/chart.js; not manifest.json',
  }),
  content: Type.String({ maxLength: 256 * 1024, description: 'UTF-8 file content' }),
})

/** @param {unknown} error */
function describeError(error) {
  const record = /** @type {{ code?: unknown, details?: { errors?: unknown } } | null} */ (
    error && typeof error === 'object' ? error : null
  )
  if (record && typeof record.code === 'string' && record.code.startsWith('component_')) {
    const errors = Array.isArray(record.details?.errors) ? record.details.errors : []
    // 结构化清单原样给代理，方便它逐项修正后重试。
    return `${record.code}${errors.length ? `\n${JSON.stringify(errors, null, 2)}` : ''}`
  }
  return error instanceof Error ? error.message : String(error)
}

/**
 * @param {{ id: string, mode: string, version: string, rev: string | null, status: string, files: { path: string, bytes: number, added: number, removed: number }[], deleted: string[], snapshot: number | null }} result
 */
function resultContent(result) {
  const lines = [
    `${result.mode === 'create' ? 'Created' : 'Updated'} component "${result.id}" v${result.version} (status ${result.status}${result.rev ? `, rev ${result.rev}` : ''}).`,
    ...result.files.map(
      (file) => `  ${file.path} +${file.added} −${file.removed} (${file.bytes} B)`,
    ),
    ...(result.deleted.length ? [`  deleted: ${result.deleted.join(', ')}`] : []),
    result.snapshot !== null
      ? `Previous version saved as snapshot ${result.snapshot}.`
      : 'No snapshot (new component).',
    'Open component studio views hot reload automatically. The component is not placed anywhere until the user chooses a placement.',
  ]
  return { content: [{ type: 'text', text: lines.join('\n') }], details: result }
}

/**
 * @param {{
 *   componentRuntime?: { writeComponent: (input: object, options: object) => Promise<any> } | null,
 *   sessionId?: string,
 * }} [context]
 */
export function createComponentCreateTool({ componentRuntime, sessionId } = {}) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: `${manifest.description} Writes <data dir>/custom-ui/<id>/ and follows the session permission mode like an ordinary file write.`,
    promptSnippet: 'Create or update a Pisper native UI component with component_create',
    promptGuidelines: PROMPT_GUIDELINES,
    parameters: Type.Object({
      mode: Type.Union([Type.Literal('create'), Type.Literal('update')], {
        description: 'create a new component directory, or update an existing native component',
      }),
      id: Type.String({
        minLength: 1,
        maxLength: 64,
        pattern: '^[a-z0-9][a-z0-9._-]{0,63}$',
        description: 'Component id = directory name (lowercase letters, digits, . _ -)',
      }),
      manifest: Type.Optional(
        Type.Object(
          {},
          {
            additionalProperties: true,
            description:
              'manifest.json object (runtime must be "native"); required for create, optional for update',
          },
        ),
      ),
      files: Type.Optional(
        Type.Array(fileSchema, {
          maxItems: 32,
          description: 'Files to write (≤32, ≤256 KiB each, ≤1 MiB total)',
        }),
      ),
      deleteFiles: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 200 }), {
          maxItems: 64,
          description: 'Component-relative paths to delete (update only)',
        }),
      ),
      summary: Type.Optional(
        Type.String({
          maxLength: 200,
          description: 'One-line change summary for the version list',
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      if (!componentRuntime?.writeComponent)
        throw new Error('Pisper component runtime is not initialized.')
      let result
      try {
        result = await componentRuntime.writeComponent(
          {
            mode: params.mode,
            id: params.id,
            ...(params.manifest !== undefined ? { manifest: params.manifest } : {}),
            files: params.files || [],
            deleteFiles: params.deleteFiles || [],
            ...(params.summary !== undefined ? { summary: params.summary } : {}),
          },
          { source: 'agent', ...(sessionId ? { sessionId } : {}) },
        )
      } catch (error) {
        throw new Error(`component_create failed: ${describeError(error)}`)
      }
      return resultContent(result)
    },
  })
}
