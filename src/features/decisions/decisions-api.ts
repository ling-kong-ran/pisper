// 决策模型 API 模块：封装 /api/decisions/* 端点与契约类型。
// 所有请求走统一 HTTP 客户端（apiJson）；apiKey 为只写字段
// （'' 表示保持现有密钥，null 表示清除），响应永不包含密钥明文。
import { apiJson } from '@/lib/api'
import { invalidResponseError } from '@/lib/http-response'
import {
  DECISION_PROVIDER_CATALOG,
  type DecisionProviderId,
} from '@shared/decision-provider-catalog.mjs'

export type DecisionRemoteProvider = DecisionProviderId
export type DecisionApprovalStatus = 'ready' | 'model_unverified' | 'threshold_required'

export type DecisionRemoteConfig = {
  provider: DecisionRemoteProvider
  baseUrl: string
  modelId: string
  hasKey: boolean
}

export type DecisionConfig = {
  remote: DecisionRemoteConfig
  delegate: DecisionDelegateConfig
  approval?: { status: DecisionApprovalStatus }
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
  model: string | null
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
      legend: Record<string, unknown> | null
      probabilities: Record<string, number>
      confidence: number | null
    }

export type DecideResult = {
  backend: 'remote'
  model: string | null
  usage?: DecisionUsage
  answers: Record<string, DecisionAnswer>
}

// Runtime 和表单共用网关默认值；协议实现与模型审批策略不进入浏览器。
export function isDecisionRemoteProvider(value: unknown): value is DecisionRemoteProvider {
  return typeof value === 'string' && Object.hasOwn(DECISION_PROVIDER_CATALOG, value)
}

export const DECISION_PROVIDER_OPTIONS =
  Object.keys(DECISION_PROVIDER_CATALOG).filter(isDecisionRemoteProvider)

export const REMOTE_PROVIDER_PRESETS = Object.fromEntries(
  Object.entries(DECISION_PROVIDER_CATALOG).map(([id, preset]) => [
    id,
    {
      baseUrl: preset.defaultBaseUrl,
      modelId: preset.defaultModelId,
    },
  ]),
)

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

// 增量 approval 状态必须校验，避免错误枚举被当成可自动审批；旧服务缺省该字段仍可读取。
export function parseDecisionsStatus(value: unknown): DecisionsStatus {
  if (!record(value) || !record(value.config)) throw invalidResponseError()
  const { remote, delegate, approval } = value.config
  if (!record(remote) || !record(delegate)) throw invalidResponseError()
  const { provider, baseUrl, modelId, hasKey } = remote
  if (
    !isDecisionRemoteProvider(provider) ||
    typeof baseUrl !== 'string' ||
    typeof modelId !== 'string' ||
    typeof hasKey !== 'boolean' ||
    typeof delegate.enabled !== 'boolean' ||
    typeof delegate.verifyActions !== 'boolean' ||
    typeof delegate.allowThreshold !== 'number' ||
    !Number.isFinite(delegate.allowThreshold) ||
    delegate.allowThreshold < 0.5 ||
    delegate.allowThreshold > 1
  )
    throw invalidResponseError()
  let status: DecisionApprovalStatus | undefined
  if (approval !== undefined) {
    if (
      !record(approval) ||
      (approval.status !== 'ready' &&
        approval.status !== 'model_unverified' &&
        approval.status !== 'threshold_required')
    )
      throw invalidResponseError()
    status = approval.status
  }
  return {
    config: {
      remote: { provider, baseUrl, modelId, hasKey },
      delegate: {
        enabled: delegate.enabled,
        verifyActions: delegate.verifyActions,
        allowThreshold: delegate.allowThreshold,
      },
      ...(status ? { approval: { status } } : {}),
    },
  }
}

export async function fetchDecisionsStatus(signal?: AbortSignal) {
  return parseDecisionsStatus(await apiJson('/api/decisions/status', { signal }))
}

export async function updateDecisionsConfig(patch: DecisionConfigPatch) {
  return parseDecisionsStatus(
    await apiJson('/api/decisions/config', {
      method: 'PUT',
      body: patch,
    }),
  )
}

export function testDecisionsConnection(signal?: AbortSignal) {
  return apiJson<DecisionTestResult>('/api/decisions/test', {
    method: 'POST',
    body: {},
    signal,
    timeout: 180_000,
  })
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
