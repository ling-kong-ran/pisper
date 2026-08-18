// 工具标签辅助：把工具 id/名称/描述映射为可翻译的展示标签。
export type ToolLabelTranslate = (key: string, values?: Record<string, unknown>) => string
export type ToolLabelSource =
  { id?: string; name?: string; description?: string } | null | undefined

// 内置工具名称 → 翻译标签（按工具 id 精确匹配，未知名回退工具名/id）。
export function toolName(tool: ToolLabelSource, t: ToolLabelTranslate) {
  if (tool?.id === 'read') return t('plugins:toolLabels.read')
  if (tool?.id === 'ls') return t('plugins:toolLabels.list')
  if (tool?.id === 'grep') return t('plugins:toolLabels.grep')
  if (tool?.id === 'find') return t('plugins:toolLabels.find')
  if (tool?.id === 'edit') return t('plugins:toolLabels.edit')
  if (tool?.id === 'write') return t('plugins:toolLabels.write')
  if (tool?.id === 'bash') return t('plugins:toolLabels.shell')
  if (tool?.id === 'web_search') return t('plugins:toolLabels.webSearch')
  if (tool?.id === 'browser_automation') return t('plugins:toolLabels.browserControl')
  if (tool?.id === 'generate_visual') return t('plugins:toolLabels.visualGeneration')
  if (tool?.id === 'skill_create') return t('plugins:toolLabels.skillCreate')
  if (tool?.id === 'plugin_create') return t('plugins:toolLabels.pluginCreate')
  if (tool?.id === 'memory_search') return t('plugins:toolLabels.memorySearch')
  if (tool?.id === 'memory_remember') return t('plugins:toolLabels.memoryRemember')
  if (tool?.id === 'mcp_list') return t('plugins:toolLabels.mcpList')
  if (tool?.id === 'mcp_manage') return t('plugins:toolLabels.mcpManage')
  return String(tool?.name || tool?.id || '')
}

// 内置工具描述 → 翻译文案（同样按 id 匹配，未知回退原始描述）。
export function toolDescription(tool: ToolLabelSource, t: ToolLabelTranslate) {
  if (tool?.id === 'read') return t('plugins:toolLabels.readDescription')
  if (tool?.id === 'ls') return t('plugins:toolLabels.listDescription')
  if (tool?.id === 'grep') return t('plugins:toolLabels.grepDescription')
  if (tool?.id === 'find') return t('plugins:toolLabels.findDescription')
  if (tool?.id === 'edit') return t('plugins:toolLabels.editDescription')
  if (tool?.id === 'write') return t('plugins:toolLabels.writeDescription')
  if (tool?.id === 'bash') return t('plugins:toolLabels.shellDescription')
  if (tool?.id === 'web_search') return t('plugins:toolLabels.webSearchDescription')
  if (tool?.id === 'browser_automation') return t('plugins:toolLabels.browserControlDescription')
  if (tool?.id === 'generate_visual') return t('plugins:toolLabels.visualGenerationDescription')
  if (tool?.id === 'skill_create') return t('plugins:toolLabels.skillCreateDescription')
  if (tool?.id === 'plugin_create') return t('plugins:toolLabels.pluginCreateDescription')
  if (tool?.id === 'memory_search') return t('plugins:toolLabels.memorySearchDescription')
  if (tool?.id === 'memory_remember') return t('plugins:toolLabels.memoryRememberDescription')
  if (tool?.id === 'mcp_list') return t('plugins:toolLabels.mcpListDescription')
  if (tool?.id === 'mcp_manage') return t('plugins:toolLabels.mcpManageDescription')
  return String(tool?.description || '')
}

// 风险等级标签：兼容中英枚举值，映射为本地化文案。
export function toolRiskLabel(risk: string | undefined, t: ToolLabelTranslate) {
  if (risk === 'high' || risk === '高风险') return t('plugins:toolLabels.highRisk')
  if (risk === 'medium' || risk === '中风险') return t('plugins:toolLabels.mediumRisk')
  if (risk === 'low' || risk === '低风险') return t('plugins:toolLabels.lowRisk')
  return String(risk || '')
}

// 工具分类标签：兼容中英分类名，映射为本地化文案。
export function toolCategoryLabel(category: string | undefined, t: ToolLabelTranslate) {
  if (category === 'filesystem' || category === '文件系统')
    return t('plugins:toolLabels.fileSystem')
  if (category === 'search' || category === '搜索') return t('plugins:toolLabels.search')
  if (category === 'terminal' || category === '终端') return t('plugins:toolLabels.terminal')
  if (category === 'visual' || category === '视觉') return t('plugins:toolLabels.visual')
  if (category === 'skills' || category === '技能') return t('plugins:toolLabels.skills')
  if (category === 'plugins' || category === '插件') return t('plugins:toolLabels.plugins')
  if (category === 'browser' || category === '浏览器') return t('plugins:toolLabels.browser')
  if (category === 'memory' || category === '星忆') return t('plugins:toolLabels.memory')
  if (category === 'mcp' || category === 'MCP') return t('plugins:toolLabels.mcp')
  if (category === 'collaboration' || category === '协作')
    return t('plugins:toolLabels.collaboration')
  return String(category || '')
}

// 工具作用域标签：内置工具映射为“影响范围”说明（当前工作区/系统用户/…），
// 未知工具回退原始 scope 字段。
export function toolScopeLabel(tool: Record<string, unknown>, t: ToolLabelTranslate) {
  if (['read', 'ls', 'grep', 'find', 'edit', 'write'].includes(String(tool.id)))
    return t('plugins:toolLabels.currentChatWorkspace')
  if (tool.id === 'bash') return t('plugins:toolLabels.currentOsUser')
  if (tool.id === 'web_search') return t('plugins:toolLabels.bingPublicWebSearch')
  if (tool.id === 'browser_automation') return t('plugins:toolLabels.isolatedSessionBrowser')
  if (tool.id === 'generate_visual') return t('plugins:toolLabels.visualProviderAndWorkspace')
  if (tool.id === 'skill_create') return t('plugins:toolLabels.projectAndGlobalSkills')
  if (tool.id === 'plugin_create') return t('plugins:toolLabels.globalPluginSources')
  if (tool.id === 'memory_search' || tool.id === 'memory_remember')
    return t('plugins:toolLabels.globalAndProjectMemory')
  if (tool.id === 'mcp_list' || tool.id === 'mcp_manage')
    return t('plugins:toolLabels.applicationMcpConfiguration')
  return String(tool.scope || '')
}

// 工具能力标签：内置工具的能力描述映射（未知回退原始 capability）。
export function toolCapabilityLabel(tool: Record<string, unknown>, t: ToolLabelTranslate) {
  if (tool.id === 'read') return t('plugins:toolLabels.readCapability')
  if (tool.id === 'ls') return t('plugins:toolLabels.listCapability')
  if (tool.id === 'grep') return t('plugins:toolLabels.grepCapability')
  if (tool.id === 'find') return t('plugins:toolLabels.findCapability')
  if (tool.id === 'edit') return t('plugins:toolLabels.editCapability')
  if (tool.id === 'write') return t('plugins:toolLabels.writeCapability')
  if (tool.id === 'bash') return t('plugins:toolLabels.shellCapability')
  if (tool.id === 'web_search') return t('plugins:toolLabels.webSearchCapability')
  if (tool.id === 'browser_automation') return t('plugins:toolLabels.browserControlCapability')
  if (tool.id === 'generate_visual') return t('plugins:toolLabels.visualGenerationCapability')
  if (tool.id === 'skill_create') return t('plugins:toolLabels.skillCreateCapability')
  if (tool.id === 'plugin_create') return t('plugins:toolLabels.pluginCreateCapability')
  if (tool.id === 'memory_search') return t('plugins:toolLabels.memorySearchCapability')
  if (tool.id === 'memory_remember') return t('plugins:toolLabels.memoryRememberCapability')
  if (tool.id === 'mcp_list') return t('plugins:toolLabels.mcpListCapability')
  if (tool.id === 'mcp_manage') return t('plugins:toolLabels.mcpManageCapability')
  return String(tool.capability || '')
}

// 是否高风险（兼容中英枚举）。
export function isHighRisk(risk: string | undefined) {
  return risk === 'high' || risk === '高风险'
}

// 是否中风险（兼容中英枚举）。
export function isMediumRisk(risk: string | undefined) {
  return risk === 'medium' || risk === '中风险'
}
