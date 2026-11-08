// 模型能力覆盖由 Provider 配置拥有，保留 kind 供旧客户端使用。
import { pisperError } from '../../shared/errors.mjs'

const CAPABILITIES = ['chat', 'image', 'video']
const LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
export function modelCapabilities(model = {}) {
  const explicit = Array.isArray(model.capabilities)
    ? CAPABILITIES.filter((value) => model.capabilities.includes(value))
    : []
  return explicit.length ? explicit : [CAPABILITIES.includes(model.kind) ? model.kind : 'chat']
}
// models.json 里的 contextWindow 同时充当 Pi 的模型定义字段（缺省时 Pi 按 128k 补），
// 所以只有 contextWindowConfigured 为 true 才算用户覆盖，否则让 pi.dev / 内置元数据决定。
// 旧版本添加模型时没有这个标记，并把 200k 占位值直接写了进去：未标记的 200k 视为未配置。
const LEGACY_CONTEXT_WINDOW_PLACEHOLDER = 200_000
export function configuredContextWindow(model) {
  const value = Number(model?.contextWindow)
  if (!(value > 0)) return 0
  if (typeof model.contextWindowConfigured === 'boolean')
    return model.contextWindowConfigured ? value : 0
  return value === LEGACY_CONTEXT_WINDOW_PLACEHOLDER ? 0 : value
}
export function updateModelOptions(existing, input) {
  const result = { ...existing }
  if (Object.hasOwn(input, 'capabilities')) {
    if (
      !Array.isArray(input.capabilities) ||
      !input.capabilities.length ||
      input.capabilities.some((value) => !CAPABILITIES.includes(value))
    )
      throw pisperError('invalid', 'model', 'capabilities: one or more of chat, image, video')
    result.capabilities = modelCapabilities(input)
    result.kind = result.capabilities[0]
  }
  if (Object.hasOwn(input, 'name')) {
    if (typeof input.name !== 'string' || input.name.trim().length > 240)
      throw pisperError('invalid', 'model', 'name: max 240 characters')
    result.name = input.name.trim() || existing.id
  }
  if (Object.hasOwn(input, 'reasoning')) {
    if (typeof input.reasoning !== 'boolean')
      throw pisperError('invalid', 'model', 'reasoning must be a boolean')
    result.reasoning = input.reasoning
  }
  if (Object.hasOwn(input, 'input')) {
    if (
      !Array.isArray(input.input) ||
      !input.input.includes('text') ||
      input.input.some((value) => !['text', 'image'].includes(value))
    )
      throw pisperError('invalid', 'model', 'input: text, optionally image')
    result.input = [...new Set(input.input)]
  }
  for (const field of ['contextWindow', 'maxTokens']) {
    if (!Object.hasOwn(input, field)) continue
    if (!Number.isSafeInteger(input[field]) || input[field] < 1 || input[field] > 100_000_000)
      throw pisperError('invalid', 'model', `${field} must be a positive integer`)
    result[field] = input[field]
    if (field === 'contextWindow') result.contextWindowConfigured = true
  }
  if (Object.hasOwn(input, 'thinkingLevels')) {
    if (
      !Array.isArray(input.thinkingLevels) ||
      input.thinkingLevels.some((level) => !LEVELS.includes(level))
    )
      throw pisperError('invalid', 'model', `thinkingLevels: any of ${LEVELS.join(', ')}`)
    // 显式用户覆盖优先于远程目录；关闭思考固定可用，未勾选等级置空。
    result.thinkingLevelMap = Object.fromEntries(
      LEVELS.map((level) => [
        level,
        level === 'off' || input.thinkingLevels.includes(level) ? level : null,
      ]),
    )
  }
  return result
}
