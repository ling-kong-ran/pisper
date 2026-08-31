// 配置类型：运行时配置（Provider/模型/端口等）与草稿/发现的类型定义。
import type { I18nValues } from '@/app/i18n'
import type { EntityRecord } from '@/types/chat'

export type Translate = (message: string, values?: I18nValues) => string
export type ProviderType = 'chat' | 'visual'

export type ProviderModel = EntityRecord & {
  id: string
  name: string
  kind: string
  api?: string
  reasoning?: boolean
  contextWindow?: number
  baseUrlOverride?: string
  added?: boolean
}

export type ProviderConfig = EntityRecord & {
  id: string
  name: string
  type: ProviderType
  api: string
  models: ProviderModel[]
  defaultModel?: string
  baseUrl?: string
  organization?: string
  enabled: boolean
  configured: boolean
  custom?: boolean
}

export type ConfigData = EntityRecord & {
  providers: ProviderConfig[]
  provider: string
  model: string
  defaultProvider?: string
  defaultModel?: string
  thinkingLevel: string
  toolMode: string
  createdProviderId?: string
  addedModelIds?: string[]
  apiKeyUpdated?: boolean
  defaultUpdated?: boolean
}

export type ConfigDraft = {
  provider: string
  providerType: ProviderType
  api: string
  model: string
  apiKey: string
  baseUrl: string
  modelBaseUrl: string
  organization: string
  thinkingLevel: string
  toolMode: string
}

export type DiscoveredProvider = EntityRecord & {
  id: string
  kind?: 'configuration' | 'authentication'
  source: string
  providerName: string
  authType?: string
  authVariable?: string
  models?: ProviderModel[]
  warnings?: Array<{ code: string }>
  imported?: boolean
  conflict?: boolean
  importable?: boolean
  api?: string
  baseUrl?: string
  location?: string
}

export type DiscoveryError = { source: string; code: string }
export type DiscoveryData = { providers: DiscoveredProvider[]; errors: DiscoveryError[] }

export type ProviderImportResult = {
  kind?: 'configuration' | 'authentication'
  config: ConfigData
  discovery: DiscoveryData
  providerId: string
  selectedModel?: string
}

export type ProviderConnectionDraft = {
  providerType: ProviderType
  api: string
  baseUrl: string
  apiKey: string
  organization: string
}

export type ModelDiscoveryResult = { models?: ProviderModel[]; config?: ConfigData }

// 视觉生成状态：运行时自动选中的图像/视频模型（与 generate_visual 的选择一致）。
export type VisualCandidate = {
  id: string
  name: string
  providerId: string
  providerName: string
  kind: string
  baseUrl: string
}

export type VisualModelStatus = {
  image: VisualCandidate | null
  video: VisualCandidate | null
  imageModels: VisualCandidate[]
  videoModels: VisualCandidate[]
  imageSelection: string
  videoSelection: string
}

// 视觉生成冒烟测试结果：产物路径 + 使用的模型 + base64 预览（可能为空）。
export type VisualTestResult = {
  path: string
  providerName: string
  modelName: string
  mimeType: string
  previewDataUrl: string
}
