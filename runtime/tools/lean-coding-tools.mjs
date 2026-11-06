// 内置编码工具的精简说明。工具定义每次请求都发给模型，Pi 自带的说明把同一条约束写了好几遍，
// 还写了输出上限（超限时工具结果自己会提示怎么继续）。这里只换文字：参数结构、校验和执行都沿用 Pi 的定义。
import {
  createEditTool,
  createFindTool,
  createGrepTool,
  createLsTool,
  createReadTool,
  createWriteTool,
} from '../runtime/pi-coding-agent.mjs'

// parameters 的键是参数路径，值是新说明；null 表示参数名已经说清楚了，去掉说明。
export const LEAN_TOOL_TEXT = Object.freeze({
  bash: {
    description:
      'Run a bash command in the working directory. Returns stdout and stderr; long output keeps the end and saves the rest to a temp file.',
    parameters: { command: 'Shell command', timeout: 'Seconds' },
  },
  read: {
    description:
      'Read a text file or image (jpg, png, gif, webp, bmp). Long files are cut off with a hint to continue via offset.',
    parameters: { path: 'File path', offset: 'First line, from 1', limit: 'Max lines' },
  },
  edit: {
    description: 'Replace exact text in one file.',
    guidelines: ['Use edit for targeted changes; put all changes to one file in a single call.'],
    parameters: {
      path: 'File path',
      edits: 'Each is matched against the original file, not applied in sequence',
      'edits[].oldText':
        'Exact text that occurs once in the file and does not overlap another edit; merge nearby changes',
      'edits[].newText': null,
    },
  },
  write: {
    description: 'Create or overwrite a file, creating parent folders.',
    guidelines: ['Use write only for new files or complete rewrites.'],
    parameters: { path: 'File path', content: null },
  },
  grep: {
    description:
      'Search file contents. Returns matching lines with paths and line numbers; respects .gitignore.',
    parameters: {
      pattern: 'Regex, or plain text with literal',
      path: 'File or folder (default: current)',
      glob: "File filter, e.g. '**/*.spec.ts'",
      ignoreCase: null,
      literal: null,
      context: 'Lines around each match',
      limit: 'Max matches (default 100)',
    },
  },
  find: {
    description: 'Find files by glob. Returns paths relative to the folder; respects .gitignore.',
    parameters: {
      pattern: "Glob, e.g. '*.ts' or 'src/**/*.spec.ts'",
      path: 'Folder (default: current)',
      limit: 'Max results (default 1000)',
    },
  },
  ls: {
    description: 'List a folder, sorted, with / after subfolders. Includes dotfiles.',
    parameters: { path: 'Folder (default: current)', limit: 'Max entries (default 500)' },
  },
})

function leanSchema(schema, texts, prefix = '') {
  if (schema?.items) return { ...schema, items: leanSchema(schema.items, texts, `${prefix}[]`) }
  if (!schema?.properties) return schema
  return {
    ...schema,
    properties: Object.fromEntries(
      Object.entries(schema.properties).map(([key, child]) => {
        const path = prefix ? `${prefix}.${key}` : key
        const next = leanSchema(child, texts, path)
        if (!(path in texts)) return [key, next]
        const { description: _description, ...rest } = next
        return [key, texts[path] ? { ...rest, description: texts[path] } : rest]
      }),
    ),
  }
}

export function leanToolDefinition(tool) {
  const text = LEAN_TOOL_TEXT[tool?.name]
  if (!text) return tool
  return {
    ...tool,
    description: text.description,
    promptSnippet: undefined,
    promptGuidelines: text.guidelines ?? [],
    parameters: leanSchema(tool.parameters, text.parameters),
  }
}

// 本机会话的 edit / write / grep / find / ls。自定义工具同名时覆盖 Pi 的内置工具；
// 是否可用仍由会话的工具清单决定，只读模式下 edit、write 不会进来。read 和 bash 各有自己的封装。
export async function createLeanCodingTools(cwd) {
  const tools = await Promise.all([
    createEditTool(cwd),
    createWriteTool(cwd),
    createGrepTool(cwd),
    createFindTool(cwd),
    createLsTool(cwd),
  ])
  return tools.map(leanToolDefinition)
}

// 子 Agent 用内置 read（父会话的 read 另有读图压缩），同样换成精简说明。
export async function createLeanReadTool(cwd) {
  return leanToolDefinition(await createReadTool(cwd))
}
