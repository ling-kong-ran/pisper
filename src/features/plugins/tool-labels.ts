export type ToolLabelTranslate = (key: string, values?: Record<string, unknown>) => string
export type ToolLabelSource =
  { id?: string; name?: string; description?: string } | null | undefined

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
  if (tool?.id === 'memory_search') return t('plugins:toolLabels.memorySearch')
  if (tool?.id === 'memory_remember') return t('plugins:toolLabels.memoryRemember')
  if (tool?.id === 'mcp_list') return t('plugins:toolLabels.mcpList')
  if (tool?.id === 'mcp_manage') return t('plugins:toolLabels.mcpManage')
  return String(tool?.name || tool?.id || '')
}

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
  if (tool?.id === 'memory_search') return t('plugins:toolLabels.memorySearchDescription')
  if (tool?.id === 'memory_remember') return t('plugins:toolLabels.memoryRememberDescription')
  if (tool?.id === 'mcp_list') return t('plugins:toolLabels.mcpListDescription')
  if (tool?.id === 'mcp_manage') return t('plugins:toolLabels.mcpManageDescription')
  return String(tool?.description || '')
}

export function toolRiskLabel(risk: string | undefined, t: ToolLabelTranslate) {
  if (risk === '高风险') return t('plugins:toolLabels.highRisk')
  if (risk === '中风险') return t('plugins:toolLabels.mediumRisk')
  if (risk === '低风险') return t('plugins:toolLabels.lowRisk')
  return String(risk || '')
}

export function toolCategoryLabel(category: string | undefined, t: ToolLabelTranslate) {
  if (category === '文件系统') return t('plugins:toolLabels.fileSystem')
  if (category === '搜索') return t('plugins:toolLabels.search')
  if (category === '终端') return t('plugins:toolLabels.terminal')
  if (category === '视觉') return t('plugins:toolLabels.visual')
  if (category === '浏览器') return t('plugins:toolLabels.browser')
  if (category === '星忆') return t('plugins:toolLabels.memory')
  if (category === 'MCP') return t('plugins:toolLabels.mcp')
  return String(category || '')
}

export function toolScopeLabel(tool: Record<string, unknown>, t: ToolLabelTranslate) {
  if (['read', 'ls', 'grep', 'find', 'edit', 'write', 'bash'].includes(String(tool.id)))
    return t('plugins:toolLabels.currentChatWorkspace')
  if (tool.id === 'web_search') return t('plugins:toolLabels.bingPublicWebSearch')
  if (tool.id === 'browser_automation') return t('plugins:toolLabels.isolatedSessionBrowser')
  if (tool.id === 'generate_visual') return t('plugins:toolLabels.visualProviderAndWorkspace')
  if (tool.id === 'memory_search' || tool.id === 'memory_remember')
    return t('plugins:toolLabels.globalAndProjectMemory')
  if (tool.id === 'mcp_list' || tool.id === 'mcp_manage')
    return t('plugins:toolLabels.applicationMcpConfiguration')
  return String(tool.scope || '')
}

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
  if (tool.id === 'memory_search') return t('plugins:toolLabels.memorySearchCapability')
  if (tool.id === 'memory_remember') return t('plugins:toolLabels.memoryRememberCapability')
  if (tool.id === 'mcp_list') return t('plugins:toolLabels.mcpListCapability')
  if (tool.id === 'mcp_manage') return t('plugins:toolLabels.mcpManageCapability')
  return String(tool.capability || '')
}
