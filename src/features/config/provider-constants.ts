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
