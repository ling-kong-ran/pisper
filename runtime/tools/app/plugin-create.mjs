// 插件创建工具：创建并安装符合规范的本地 Pisper 插件。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { PLUGIN_CREATE, PLUGIN_TOOL_DEFINITION } from '../../prompts/tools/plugin-create.mjs'

export const manifest = {
  id: 'plugin_create',
  name: 'Plugin Create',
  category: 'plugins',
  risk: 'high',
  description: PLUGIN_CREATE.description,
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
    description: PLUGIN_TOOL_DEFINITION.params.name,
  }),
  label: Type.Optional(
    Type.String({ minLength: 1, maxLength: 100, description: PLUGIN_TOOL_DEFINITION.params.label }),
  ),
  description: Type.String({
    minLength: 1,
    maxLength: 1000,
    description: PLUGIN_TOOL_DEFINITION.params.description,
  }),
  scope: Type.Optional(
    Type.String({
      minLength: 1,
      maxLength: 500,
      description: PLUGIN_TOOL_DEFINITION.params.scope,
    }),
  ),
  parameters: Type.Object(
    {},
    {
      additionalProperties: true,
      description: PLUGIN_TOOL_DEFINITION.params.parameters,
    },
  ),
})

export function createPluginCreateTool({ pluginRuntime, onPluginsChanged } = {}) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: PLUGIN_CREATE.toolDescription,
    promptSnippet: PLUGIN_CREATE.snippet,
    promptGuidelines: PLUGIN_CREATE.guidelines,
    parameters: Type.Object({
      id: Type.String({
        minLength: 1,
        maxLength: 96,
        pattern: '^[a-z0-9](?:[a-z0-9.-]{0,94}[a-z0-9])?$',
        description: PLUGIN_CREATE.params.id,
      }),
      name: Type.String({ minLength: 1, maxLength: 100, description: PLUGIN_CREATE.params.name }),
      version: Type.Optional(
        Type.String({
          pattern: '^\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?$',
          description: PLUGIN_CREATE.params.version,
        }),
      ),
      description: Type.Optional(
        Type.String({ maxLength: 1000, description: PLUGIN_CREATE.params.description }),
      ),
      permissions: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 100 }), {
          maxItems: 32,
          description: PLUGIN_CREATE.params.permissions,
        }),
      ),
      tools: Type.Array(toolDefinition, {
        minItems: 1,
        maxItems: 32,
        description: PLUGIN_CREATE.params.tools,
      }),
      entryCode: Type.String({
        minLength: 1,
        maxLength: 2_000_000,
        description: PLUGIN_CREATE.params.entryCode,
      }),
      files: Type.Optional(
        Type.Array(
          Type.Object({
            path: Type.String({
              minLength: 1,
              maxLength: 500,
              description: PLUGIN_CREATE.params.path,
            }),
            content: Type.String({
              maxLength: 2_000_000,
              description: PLUGIN_CREATE.params.content,
            }),
          }),
          { maxItems: 64, description: PLUGIN_CREATE.params.files },
        ),
      ),
    }),
    async execute(_toolCallId, params) {
      if (!pluginRuntime?.create) throw new Error('Pisper plugin runtime is not initialized.')
      const result = await pluginRuntime.create(params)
      await onPluginsChanged?.(result)
      return resultContent(result)
    },
  })
}
