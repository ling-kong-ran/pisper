// 组件创建工具：代理新建或更新 Pisper 原生 UI 组件（<数据目录>/custom-ui/<id>/，规格 5.2、计划 §5.1）。
// 校验与写入都在 CustomUiService.writeComponent（模板创建共用同一份），这里只做参数 schema、
// 结构化错误包装与结果摘要。权限按普通写文件处理：跟随会话权限模式，不算 critical（负责人决定）。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { COMPONENT_CREATE } from '../../prompts/tools/component-create.mjs'

export const manifest = {
  id: 'component_create',
  name: 'Component Create',
  category: 'plugins',
  risk: 'high',
  description: COMPONENT_CREATE.description,
  scope: 'Pisper data directory custom-ui/<id>',
  capability:
    'Write component manifest, JavaScript, CSS, JSON and SVG files with a version snapshot before each update; hot reloads open views',
  source: 'app',
}

const fileSchema = Type.Object({
  path: Type.String({
    minLength: 1,
    maxLength: 200,
    description: COMPONENT_CREATE.params.path,
  }),
  content: Type.String({ maxLength: 256 * 1024, description: COMPONENT_CREATE.params.content }),
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
    description: COMPONENT_CREATE.toolDescription,
    promptSnippet: COMPONENT_CREATE.snippet,
    promptGuidelines: COMPONENT_CREATE.guidelines,
    parameters: Type.Object({
      mode: Type.Union([Type.Literal('create'), Type.Literal('update')], {
        description: COMPONENT_CREATE.params.mode,
      }),
      id: Type.String({
        minLength: 1,
        maxLength: 64,
        pattern: '^[a-z0-9][a-z0-9._-]{0,63}$',
        description: COMPONENT_CREATE.params.id,
      }),
      manifest: Type.Optional(
        Type.Object(
          {},
          {
            additionalProperties: true,
            description: COMPONENT_CREATE.params.manifest,
          },
        ),
      ),
      files: Type.Optional(
        Type.Array(fileSchema, {
          maxItems: 32,
          description: COMPONENT_CREATE.params.files,
        }),
      ),
      deleteFiles: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 200 }), {
          maxItems: 64,
          description: COMPONENT_CREATE.params.deleteFiles,
        }),
      ),
      summary: Type.Optional(
        Type.String({
          maxLength: 200,
          description: COMPONENT_CREATE.params.summary,
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
