// 工具注册表：合并内置工具目录与 Pisper 应用工具目录，
// 提供按配置/预设解析启用工具集与工具定义创建的入口。
import { APP_TOOL_CATALOG, createAppToolDefinitions, createMultiAgentTools } from './app/index.mjs'
import { BUILTIN_TOOL_CATALOG, TOOL_PRESETS } from './builtin-catalog.mjs'

export { TOOL_PRESETS, createMultiAgentTools }

export const TOOL_CATALOG = [...BUILTIN_TOOL_CATALOG, ...APP_TOOL_CATALOG]

const TOOL_IDS = new Set(TOOL_CATALOG.map((tool) => tool.id))

// 从配置中解析启用工具：配置缺省时按 toolMode 预设回退到 full。
// 只保留目录中存在的工具 ID。
export function toolsFromConfig(config = {}) {
  const configured = Array.isArray(config.enabledTools)
    ? config.enabledTools.filter((tool) => TOOL_IDS.has(tool))
    : null
  return configured || TOOL_PRESETS[config.toolMode] || TOOL_PRESETS.full
}

export function presetFromTools(enabledTools) {
  return (
    Object.entries(TOOL_PRESETS).find(
      ([, tools]) =>
        tools.length === enabledTools.length && tools.every((tool) => enabledTools.includes(tool)),
    )?.[0] || 'custom'
  )
}

export function sanitizeEnabledTools(enabledTools) {
  return [
    ...new Set(
      Array.isArray(enabledTools) ? enabledTools.filter((tool) => TOOL_IDS.has(tool)) : [],
    ),
  ]
}

export function createAppTools({ enabledTools, ...context }) {
  return createAppToolDefinitions({ ...context, enabledTools: sanitizeEnabledTools(enabledTools) })
}
