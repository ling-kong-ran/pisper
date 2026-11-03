// 实验功能接口：GET / PATCH /api/settings/beta-features。响应按不可信数据逐项校验，结构不对整体拒绝。
import {
  BETA_FEATURE_STAGES,
  isBetaFeatureId,
  type BetaFeatureState,
} from '@shared/beta-features.mjs'
import { requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

const ENDPOINT = '/api/settings/beta-features'
// 反馈链接只放行 GitHub：链接会以外链打开，不让 Runtime 返回的任意地址出现在界面上。
const FEEDBACK_URL = /^https:\/\/github\.com\/[^\s]{1,400}$/

function parseFeature(value: unknown): BetaFeatureState {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  const input = value as Record<string, unknown>
  if (
    !isBetaFeatureId(input.id) ||
    !BETA_FEATURE_STAGES.includes(input.stage as BetaFeatureState['stage']) ||
    typeof input.enabled !== 'boolean' ||
    typeof input.available !== 'boolean' ||
    (input.unavailableReason !== null && input.unavailableReason !== 'runtime')
  )
    throw invalidResponseError()
  return {
    id: input.id as string,
    stage: input.stage as BetaFeatureState['stage'],
    feedbackUrl:
      typeof input.feedbackUrl === 'string' && FEEDBACK_URL.test(input.feedbackUrl)
        ? input.feedbackUrl
        : '',
    enabled: input.enabled,
    available: input.available,
    unavailableReason: input.unavailableReason,
  }
}

export function parseBetaFeatures(value: unknown): BetaFeatureState[] {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  const features = (value as Record<string, unknown>).features
  if (!Array.isArray(features)) throw invalidResponseError()
  return features.map(parseFeature)
}

export function fetchBetaFeatures(options: { signal?: AbortSignal } = {}) {
  return requestJson(ENDPOINT, { signal: options.signal, parse: parseBetaFeatures })
}

export function updateBetaFeature(id: string, enabled: boolean) {
  return requestJson(ENDPOINT, {
    method: 'PATCH',
    data: { [id]: enabled },
    parse: parseBetaFeatures,
  })
}
