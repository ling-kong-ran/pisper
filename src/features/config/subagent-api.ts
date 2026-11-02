// 子代理的领域接口：模型档位映射随 /api/config 一起返回，定义文件由 Runtime 读取后列出。
import { apiJson } from '@/lib/api'
import type { ConfigData, SubagentModelTier } from './config-types'

export type AgentProfile = {
  name: string
  description: string
  // 具体模型 "provider/modelId"；没写时为空。
  modelRef: string
  // 档位；写了具体模型时是它用不了时的退路。
  tier: SubagentModelTier | ''
  thinkingLevel: string
  tools: string[] | null
  scope: 'project' | 'global'
  // 相对位置（.pisper/agents/x.md 或 agents/x.md），不含个人目录。
  file: string
  overrides?: string
}

export type AgentProfileError = { scope: 'project' | 'global'; file: string; message: string }

export type AgentProfileList = { profiles: AgentProfile[]; errors: AgentProfileError[] }

export const subagentApi = {
  // 不带项目时只列全局定义；项目里的定义在该项目的会话里生效。
  listProfiles() {
    return apiJson<AgentProfileList>('/api/agent-profiles')
  },
  // 空字符串表示该档位跟随当前会话模型。
  saveModelTiers(tiers: Partial<Record<SubagentModelTier, string>>) {
    return apiJson<ConfigData>('/api/config/subagent-model-tiers', {
      method: 'PUT',
      body: JSON.stringify({ tiers }),
    })
  },
}
