// 内置编码工具的精简说明。工具定义每次请求都发给模型，Pi 自带的说明把同一条约束写了好几遍，
// 还写了输出上限（超限时工具结果自己会提示怎么继续）。这里只换文字：参数结构、校验和执行都沿用 Pi 的定义；
// 文字本身在 runtime/prompts/tools/lean-coding-tools.mjs。
import {
  createEditTool,
  createFindTool,
  createGrepTool,
  createLsTool,
  createReadTool,
  createWriteTool,
} from '../runtime/pi-coding-agent.mjs'
import { LEAN_TOOL_TEXT } from '../prompts/tools/lean-coding-tools.mjs'

export { LEAN_TOOL_TEXT }

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
