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

// Multi-agent tools are internal runtime tools (like goal/plan) and are intentionally
// omitted from the plugins catalog so they stay hidden from the frontend tool list.
export const APP_TOOL_CATALOG = [
  webSearchManifest,
  browserAutomationManifest,
  visualGenerateManifest,
  skillCreateManifest,
  ...memoryManifests,
  ...mcpManifests,
]
export { createMultiAgentTools }

const APP_TOOL_FACTORIES = {
  [webSearchManifest.id]: createWebSearchTool,
  [browserAutomationManifest.id]: createBrowserAutomationTool,
  [visualGenerateManifest.id]: createVisualGenerateTool,
  [skillCreateManifest.id]: createSkillCreateTool,
  ...memoryFactories,
  ...mcpFactories,
}

export function createAppToolDefinitions({ enabledTools, ...context }) {
  return enabledTools
    .filter((id) => APP_TOOL_FACTORIES[id])
    .map((id) => APP_TOOL_FACTORIES[id](context))
}
