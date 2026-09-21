// 决策模型（Jev）API 模块：封装 /api/decisions/* 端点与契约类型。
// 所有请求走统一 HTTP 客户端（apiJson）；apiKey 为只写字段
// （'' 表示保持现有密钥，null 表示清除），响应永不包含密钥明文。
import { apiJson } from '@/lib/api'

export type DecisionRemoteProvider = 'typesafe' | 'openrouter' | 'custom'

export type DecisionRemoteConfig = {
  provider: DecisionRemoteProvider
  baseUrl: string
  modelId: string
  hasKey: boolean
}

export type DecisionConfig = {
  remote: DecisionRemoteConfig
  delegate: DecisionDelegateConfig
}

export type DecisionDelegateConfig = {
  enabled: boolean
  allowThreshold: number
  verifyActions: boolean
}

export type DecisionsStatus = {
  config: DecisionConfig
}

export type DecisionConfigPatch = {
  remote?: {
    provider?: DecisionRemoteProvider
    baseUrl?: string
    modelId?: string
    apiKey?: string | null
  }
  delegate?: Partial<DecisionDelegateConfig>
}

export type DecisionUsage = {
  inputTokens?: number
  costUsd?: number | null
}

export type DecisionTestResult = {
  backend: 'remote'
  ok: true
  model: string
  usage?: DecisionUsage
}

export type DecisionQuestionType = 'noul' | 'choice' | 'score'

export type DecisionQuestionInput = {
  id?: string
  type: DecisionQuestionType
  instructions: string
  options?: string[]
}

export type DecisionAnswer =
  | { type: 'noul'; noul: number }
  | {
      type: 'choice'
      choice: string
      probabilities: Record<string, number>
      confidence: number | null
    }
  | {
      type: 'score'
      score: number
      legend: Record<string, string>
      probabilities: Record<string, number>
      confidence: number | null
    }

export type DecideResult = {
  backend: 'remote'
  model: string
  usage?: DecisionUsage
  answers: Record<string, DecisionAnswer>
}

// 与 runtime REMOTE_PROVIDERS 预设保持一致，仅用于表单占位与切换默认值；
// 真正生效的地址/模型以后端 config 为准。
export const REMOTE_PROVIDER_PRESETS: Record<
  DecisionRemoteProvider,
  { baseUrl: string; modelId: string }
> = {
  typesafe: { baseUrl: 'https://api.typesafe.ai', modelId: 'jev-1.13.0' },
  openrouter: { baseUrl: 'https://openrouter.ai/api', modelId: 'typesafe/jev-1.13' },
  custom: { baseUrl: '', modelId: 'typesafe/jev-1.13' },
}

export function fetchDecisionsStatus(signal?: AbortSignal) {
  return apiJson<DecisionsStatus>('/api/decisions/status', { signal })
}

export function updateDecisionsConfig(patch: DecisionConfigPatch) {
  return apiJson<{ config: DecisionConfig }>('/api/decisions/config', {
    method: 'PUT',
    body: patch,
  })
}

export function testDecisionsConnection() {
  return apiJson<DecisionTestResult>('/api/decisions/test', { method: 'POST', body: {} })
}

export function runDecision(state: string, questions: DecisionQuestionInput[]) {
  return apiJson<DecideResult>('/api/decisions/decide', {
    method: 'POST',
    body: { state, questions },
    // 远端首次请求可能包含冷启动，放宽默认 30s 超时。
    timeout: 180_000,
  })
}

// 把任意异常归一化为可展示文案，供各卡片错误提示与 notify 复用。
export function decisionErrorMessage(error: unknown, fallback: string) {
  if (error instanceof Error && error.message) return error.message
  return fallback
}
