// 工具激活辅助：热门工具集合与 schema-only 工具定义清洗（去掉 prompt 指南字段）。
import { PLAN_TOOL_NAMES } from './app/plan-tool-names.mjs'

const HOT_TOOL_SET = new Set([
  'read',
  'grep',
  'find',
  'ls',
  'edit',
  'write',
  'bash',
  ...PLAN_TOOL_NAMES,
  'discover_tools',
  'call_tool',
  'spawn_agent',
  'list_agents',
  'send_message',
  'followup_task',
  'wait_agent',
  'interrupt_agent',
])

export function schemaOnlyToolDefinition(tool) {
  const guidelines = Array.isArray(tool?.promptGuidelines)
    ? tool.promptGuidelines.map((item) => String(item || '').trim()).filter(Boolean)
    : []
  return {
    ...tool,
    description: [String(tool?.description || '').trim(), ...guidelines].filter(Boolean).join('\n'),
    promptSnippet: undefined,
    promptGuidelines: [],
  }
}

export function schemaOnlyToolDefinitions(tools = []) {
  return tools.map(schemaOnlyToolDefinition)
}

export function hotToolNames(availableToolNames = []) {
  return availableToolNames.filter((name) => HOT_TOOL_SET.has(name))
}

export function mergePromotedToolNames({
  availableToolNames = [],
  promotedToolNames = [],
  requestedToolNames = [],
} = {}) {
  const available = new Set(availableToolNames)
  return [...new Set([...promotedToolNames, ...requestedToolNames])].filter(
    (name) => available.has(name) && !HOT_TOOL_SET.has(name),
  )
}

export function selectedToolNames({
  availableToolNames = [],
  promotedToolNames = [],
  requestedToolNames = [],
  goalToolNames = [],
  goalActive = false,
} = {}) {
  const available = new Set(availableToolNames)
  const names = new Set(hotToolNames(availableToolNames))
  for (const name of promotedToolNames) if (available.has(name)) names.add(name)
  for (const name of requestedToolNames) if (available.has(name)) names.add(name)
  if (goalActive) for (const name of goalToolNames) if (available.has(name)) names.add(name)
  return [...names]
}
