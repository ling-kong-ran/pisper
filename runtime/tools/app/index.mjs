// 应用工具目录与工厂注册：汇总 Pisper 自研工具（记忆/MCP/搜索/浏览器/视觉/技能/插件）
// 的清单与创建工厂，按启用列表生成工具定义。
// 多 Agent 工具是内部运行时工具（与 goal/plan 一样），刻意不进插件目录，
// 避免在前端工具列表中出现。
import { createVisualGenerateTool, manifest as visualGenerateManifest } from './visual-generate.mjs'
import { factories as memoryFactories, manifests as memoryManifests } from './memory.mjs'
import { createMultiAgentTools } from './multi-agent.mjs'
import { factories as mcpFactories, manifests as mcpManifests } from './mcp-management.mjs'
import { createWebSearchTool, manifest as webSearchManifest } from './web-search.mjs'
import {
  createBrowserAutomationTool,
  manifest as browserAutomationManifest,
} from './browser-automation.mjs'
import { createSkillCreateTool, manifest as skillCreateManifest } from './skill-create.mjs'
import { createPluginCreateTool, manifest as pluginCreateManifest } from './plugin-create.mjs'

// Multi-agent tools are internal runtime tools (like goal/plan) and are intentionally
// omitted from the plugins catalog so they stay hidden from the frontend tool list.
export const APP_TOOL_CATALOG = [
  webSearchManifest,
  browserAutomationManifest,
  visualGenerateManifest,
  skillCreateManifest,
  pluginCreateManifest,
  ...memoryManifests,
  ...mcpManifests,
]
export { createMultiAgentTools }

const APP_TOOL_FACTORIES = {
  [webSearchManifest.id]: createWebSearchTool,
  [browserAutomationManifest.id]: createBrowserAutomationTool,
  [visualGenerateManifest.id]: createVisualGenerateTool,
  [skillCreateManifest.id]: createSkillCreateTool,
  [pluginCreateManifest.id]: createPluginCreateTool,
  ...memoryFactories,
  ...mcpFactories,
}

export function createAppToolDefinitions({ enabledTools, ...context }) {
  return enabledTools
    .filter((id) => APP_TOOL_FACTORIES[id])
    .map((id) => APP_TOOL_FACTORIES[id](context))
}
