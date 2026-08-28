// 推荐对话模型：Provider 记录的默认模型优先，否则取目录排序后的第一个
// chat 模型（后端已按旗舰/推理能力排序）。快速配置向导预选使用。
import type { ProviderConfig } from './config-types'

export function recommendedChatModel(provider?: ProviderConfig | null) {
  const chatModels = provider?.models.filter((item) => item.kind === 'chat') || []
  return chatModels.find((item) => item.id === provider?.defaultModel) || chatModels[0] || null
}
