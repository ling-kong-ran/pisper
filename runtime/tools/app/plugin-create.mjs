// 插件创建工具：创建并安装符合规范的本地 Pisper 插件。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'

export const manifest = {
  id: 'plugin_create',
  name: 'Plugin Create',
  category: 'plugins',
  risk: 'high',
  description: 'Create and install a standards-compliant local Pisper plugin for all projects.',
  scope: 'Global Pisper plugin sources and local plugin installation',
  capability:
    'Define one or more Agent tools, create their source without overwriting files, validate the plugin, and install it',
  source: 'app',
}

function resultContent(result) {
  return {
    content: [
      {
        type: 'text',
        text: [
          `Created and installed plugin "${result.name}" (${result.id}@${result.version}).`,
          `Source: ${result.sourcePath}`,
          `Tools: ${result.tools.join(', ')}`,
          'The tools will be available to all projects on the next Agent turn through discover_tools and call_tool.',
        ].join('\n'),
      },
    ],
    details: result,
  }
}

const toolDefinition = Type.Object({
  name: Type.String({
    minLength: 1,
    maxLength: 64,
    pattern: '^[a-z][a-z0-9_]{0,63}$',
    description: 'Stable Agent tool name using lowercase letters, numbers, and underscores',
  }),
  label: Type.Optional(
    Type.String({ minLength: 1, maxLength: 100, description: 'Short user-facing tool label' }),
  ),
  description: Type.String({
    minLength: 1,
    maxLength: 1000,
    description: 'Specific description of what the tool does and when Agent should call it',
  }),
  scope: Type.Optional(
    Type.String({
      minLength: 1,
      maxLength: 500,
      description: 'Files, services, or external systems the tool can affect',
    }),
  ),
  parameters: Type.Object(
    {},
    {
      additionalProperties: true,
      description: 'JSON Schema object for the tool arguments; type must be object',
    },
  ),
})

export function createPluginCreateTool({ pluginRuntime, executionMode, onPluginsChanged } = {}) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: 'Create, validate, and install a globally available local Pisper plugin',
    promptGuidelines: [
      'Use plugin_create when the user explicitly asks to create and install a reusable Pisper plugin or new Agent tool. Do not use it for ordinary scripts or one-off code changes.',
      'A plugin is one installable unit that may provide multiple related tools. Define every tool with a precise description, object JSON Schema, and honest scope.',
      'entryCode must export async function execute({ toolName, arguments: input, context }). Return a string or a Pi tool result with a content array.',
      'Use context.cwd for the current workspace, context.sessionId for the chat identity, and context.dataDir for persistent plugin-owned data.',
      'The first local-plugin version supports Node.js built-ins and relative bundled files. Do not depend on npm installation, lifecycle scripts, native modules, Provider injection, TUI UI, or overriding built-in tools.',
      'Plugin code runs as the current operating-system user in an isolated Worker, not an OS sandbox. Minimize file and network access and never embed credentials.',
      'The tool writes to the global Pisper plugin-sources/<plugin-id> directory, refuses to overwrite source or installed plugins, validates the same manifest used by the Plugins page, and installs the plugin for all projects from the next Agent turn.',
    ],
    parameters: Type.Object({
      id: Type.String({
        minLength: 1,
        maxLength: 96,
        pattern: '^[a-z0-9](?:[a-z0-9.-]{0,94}[a-z0-9])?$',
        description: 'Stable plugin id using lowercase letters, numbers, dots, and hyphens',
      }),
      name: Type.String({ minLength: 1, maxLength: 100, description: 'User-facing plugin name' }),
      version: Type.Optional(
        Type.String({
          pattern: '^\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?$',
          description: 'Semantic version; defaults to 1.0.0',
        }),
      ),
      description: Type.Optional(
        Type.String({ maxLength: 1000, description: 'Plugin-level purpose and behavior' }),
      ),
      permissions: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 100 }), {
          maxItems: 32,
          description: 'Human-readable permission declarations shown during review',
        }),
      ),
      tools: Type.Array(toolDefinition, {
        minItems: 1,
        maxItems: 32,
        description: 'Agent tools provided by this plugin',
      }),
      entryCode: Type.String({
        minLength: 1,
        maxLength: 2_000_000,
        description: 'Complete ESM source for index.mjs implementing execute()',
      }),
      files: Type.Optional(
        Type.Array(
          Type.Object({
            path: Type.String({
              minLength: 1,
              maxLength: 500,
              description: 'Plugin-relative path; cannot replace index.mjs or pisper-plugin.json',
            }),
            content: Type.String({
              maxLength: 2_000_000,
              description: 'UTF-8 text file content',
            }),
          }),
          { maxItems: 64, description: 'Optional relative JavaScript, JSON, or text files' },
        ),
      ),
    }),
    async execute(_toolCallId, params) {
      if (executionMode !== 'full-access') {
        throw new Error('plugin_create 仅在“完全访问”执行模式下可用。')
      }
      if (!pluginRuntime?.create) throw new Error('Pisper plugin runtime is not initialized.')
      const result = await pluginRuntime.create(params)
      await onPluginsChanged?.(result)
      return resultContent(result)
    },
  })
}
