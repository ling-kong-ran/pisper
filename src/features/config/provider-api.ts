// Provider 编辑页只通过领域接口提交草稿，凭据不写入浏览器存储。
import { apiJson } from '@/lib/api'
import type { ConfigData, DiscoveryData, ProviderModel } from './config-types'
export type ModelOptionsDraft = {
  modelId: string
  name: string
  capabilities: NonNullable<ProviderModel['capabilities']>
  input: string[]
  reasoning: boolean
  thinkingLevels?: string[]
  // 省略表示沿用元数据推断的上下文窗口
  contextWindow?: number
  maxTokens: number
}
export type LocalProviderImportResult = {
  config: ConfigData
  discovery: DiscoveryData
  imported: Array<{ id: string; providerId: string; source: string }>
  skipped: Array<{ id: string; source: string; reason: string }>
}
export const providerApi = {
  // 备用模型按顺序保存为 "provider/modelId"；空数组表示不使用备用模型。
  saveFallbackModels(models: string[]) {
    return apiJson<ConfigData>('/api/config/fallback-models', {
      method: 'PUT',
      body: JSON.stringify({ models }),
    })
  },
  // 辅助模型保存为 "provider/modelId"；空字符串表示不设置。
  saveAuxiliaryModel(model: string) {
    return apiJson<ConfigData>('/api/config/auxiliary-model', {
      method: 'PUT',
      body: JSON.stringify({ model }),
    })
  },
  deleteModel(providerId: string, modelId: string) {
    return apiJson<ConfigData>('/api/providers/' + encodeURIComponent(providerId) + '/models', {
      method: 'DELETE',
      body: JSON.stringify({ modelId }),
    })
  },
  saveModel(providerId: string, draft: ModelOptionsDraft, create = false) {
    return apiJson<ConfigData>(
      '/api/providers/' + encodeURIComponent(providerId) + '/models' + (create ? '' : '/options'),
      {
        method: create ? 'POST' : 'PUT',
        body: JSON.stringify(
          create ? { ...draft, id: draft.modelId, kind: draft.capabilities[0] } : draft,
        ),
      },
    )
  },
  saveConnection(
    providerId: string,
    draft: { name: string; api: string; baseUrl: string; apiKey?: string },
  ) {
    return apiJson<ConfigData>('/api/providers/' + encodeURIComponent(providerId) + '/connection', {
      method: 'PUT',
      body: JSON.stringify(draft),
    })
  },
  importLocal() {
    return apiJson<LocalProviderImportResult>('/api/providers/import-local', {
      method: 'POST',
      body: '{}',
    })
  },
}
