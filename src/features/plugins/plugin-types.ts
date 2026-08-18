// 插件领域类型：插件能力（工具）的声明与运行时状态。
import type { EntityRecord } from '@/types/chat'

export type PluginCapability = EntityRecord & {
  name: string
  label: string
  enabled: boolean
  risk: string
  effectiveRisk?: string
  category: string
  description: string
  scope?: string
}

export type InstalledPlugin = EntityRecord & {
  id: string
  name: string
  description: string
  version: string
  source: 'builtin' | 'local'
  builtIn: boolean
  enabled: boolean
  capabilities: PluginCapability[]
  permissions?: string[]
  systemAccess?: boolean
  installedAt?: string
}

export type WebSearchSettings = {
  provider: string
  language: string
  safeSearch: number
  maxResults: number
}

export type PluginChange = EntityRecord & {
  timestamp: string
  tool: string
  enabled: boolean
}

export type PluginsData = EntityRecord & {
  plugins: InstalledPlugin[]
  enabledTools: string[]
  callableToolNames?: string[]
  presets: Record<string, string[]>
  webSearch: WebSearchSettings
  changes: PluginChange[]
  preset?: string
}

export type PluginInspection = {
  inspectionId: string
  plugin: InstalledPlugin
  fileCount: number
  byteCount: number
  digest: string
  warnings: string[]
}
