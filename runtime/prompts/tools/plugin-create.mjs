// plugin_create 的说明。

export const PLUGIN_CREATE = Object.freeze({
  description: 'Create and install a standards-compliant local Pisper plugin for all projects.',
  toolDescription:
    'Create and install a standards-compliant local Pisper plugin for all projects. Needs user approval unless the session has full access.',
  snippet:
    'Create, validate, and install a globally available local Pisper plugin with plugin_create',
  guidelines: [
    'Use plugin_create directly when the user explicitly asks to create and install a reusable Pisper plugin or new Agent tool. Do not substitute ordinary scripts or one-off code changes.',
    'A plugin is one installable unit that may provide multiple related tools. Define every tool with a precise description, object JSON Schema, and honest scope.',
    'entryCode must export async function execute({ toolName, arguments: input, context }). Return a string or a Pi tool result with a content array.',
    'Use context.cwd for the current workspace, context.sessionId for the chat identity, and context.dataDir for persistent plugin-owned data.',
    'The first local-plugin version supports Node.js built-ins and relative bundled files. Do not depend on npm installation, lifecycle scripts, native modules, Provider injection, TUI UI, or overriding built-in tools.',
    'Plugin code runs as the current operating-system user in an isolated Worker, not an OS sandbox. Minimize file and network access and never embed credentials.',
    'A local plugin only adds tools, and every call goes through Pisper approval. When the request needs to change Agent behavior itself (intercept tool calls, rewrite prompts, add commands or providers), tell the user that needs a Pi Extension installed from the Plugins page instead of plugin_create.',
    'The tool writes to the global Pisper plugin-sources/<plugin-id> directory, refuses to overwrite source or installed plugins, validates the same manifest used by the Plugins page, and installs the plugin for all projects from the next Agent turn.',
  ],
  params: {
    id: 'Stable plugin id using lowercase letters, numbers, dots, and hyphens',
    name: 'User-facing plugin name',
    version: 'Semantic version; defaults to 1.0.0',
    description: 'Plugin-level purpose and behavior',
    permissions: 'Human-readable permission declarations shown during review',
    tools: 'Agent tools provided by this plugin',
    entryCode: 'Complete ESM source for index.mjs implementing execute()',
    path: 'Plugin-relative path; cannot replace index.mjs or pisper-plugin.json',
    content: 'UTF-8 text file content',
    files: 'Optional relative JavaScript, JSON, or text files',
  },
})

export const PLUGIN_TOOL_DEFINITION = Object.freeze({
  params: {
    name: 'Stable Agent tool name using lowercase letters, numbers, and underscores',
    label: 'Short user-facing tool label',
    description: 'Specific description of what the tool does and when Agent should call it',
    scope: 'Files, services, or external systems the tool can affect',
    parameters: 'JSON Schema object for the tool arguments; type must be object',
  },
})
