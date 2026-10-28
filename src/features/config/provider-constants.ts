// Provider API 常量：API 形态（responses/completions 等）与标准端点、
// 模型档位（轻量/均衡/推理）的展示名。
import { Bot, Brain, Code2, Network, Sparkles, Zap } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'

export const PROVIDER_APIS: Array<[string, string]> = [
  ['openai-responses', 'OpenAI Responses'],
  ['openai-completions', 'OpenAI Chat Completions'],
  ['anthropic-messages', 'Anthropic Messages'],
  ['google-generative-ai', 'Google Generative AI'],
]

// 内置 Provider 可能使用自定义连接不提供的专用协议；编辑时仍要显示当前值，否则下拉框为空。
const BUILT_IN_PROVIDER_API_LABELS: Record<string, string> = {
  'openai-codex-responses': 'OpenAI Codex Responses',
  'azure-openai-responses': 'Azure OpenAI Responses',
  'mistral-conversations': 'Mistral Conversations',
  'bedrock-converse-stream': 'Amazon Bedrock Converse',
  'google-vertex': 'Google Vertex AI',
  'pi-messages': 'Pi Messages',
}

export function providerApiOptions(current: string): Array<[string, string]> {
  if (!current || PROVIDER_APIS.some(([value]) => value === current)) return PROVIDER_APIS
  return [...PROVIDER_APIS, [current, BUILT_IN_PROVIDER_API_LABELS[current] || current]]
}

// 已知 Provider 的展示图标（未知/自定义 Provider 由调用方回退到 Server）。
export const PROVIDER_ICONS: Record<string, LucideIcon> = {
  openai: Bot,
  'openai-codex': Bot,
  anthropic: Brain,
  google: Sparkles,
  deepseek: Code2,
  xai: Zap,
  openrouter: Network,
  'kimi-coding': Sparkles,
  'zai-coding-cn': Brain,
}
