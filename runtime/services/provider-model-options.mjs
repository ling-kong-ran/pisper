// 模型能力覆盖由 Provider 配置拥有，保留 kind 供旧客户端使用。
const CAPABILITIES = ['chat', 'image', 'video']
const LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
export function modelCapabilities(model = {}) {
  const explicit = Array.isArray(model.capabilities)
    ? CAPABILITIES.filter((value) => model.capabilities.includes(value))
    : []
  return explicit.length ? explicit : [CAPABILITIES.includes(model.kind) ? model.kind : 'chat']
}
export function updateModelOptions(existing, input) {
  const result = { ...existing }
  if (Object.hasOwn(input, 'capabilities')) {
    if (
      !Array.isArray(input.capabilities) ||
      !input.capabilities.length ||
      input.capabilities.some((value) => !CAPABILITIES.includes(value))
    )
      throw new Error('请至少选择一种有效的模型能力。')
    result.capabilities = modelCapabilities(input)
    result.kind = result.capabilities[0]
  }
  if (Object.hasOwn(input, 'name')) {
    if (typeof input.name !== 'string' || input.name.trim().length > 240)
      throw new Error('模型名称无效。')
    result.name = input.name.trim() || existing.id
  }
  if (Object.hasOwn(input, 'reasoning')) {
    if (typeof input.reasoning !== 'boolean') throw new Error('思考能力必须为布尔值。')
    result.reasoning = input.reasoning
  }
  if (Object.hasOwn(input, 'input')) {
    if (
      !Array.isArray(input.input) ||
      !input.input.includes('text') ||
      input.input.some((value) => !['text', 'image'].includes(value))
    )
      throw new Error('模型输入必须包含文本，可同时包含图片。')
    result.input = [...new Set(input.input)]
  }
  for (const field of ['contextWindow', 'maxTokens']) {
    if (!Object.hasOwn(input, field)) continue
    if (!Number.isSafeInteger(input[field]) || input[field] < 1 || input[field] > 100_000_000)
      throw new Error('上下文和输出 Token 数必须为有效正整数。')
    result[field] = input[field]
  }
  if (Object.hasOwn(input, 'thinkingLevels')) {
    if (
      !Array.isArray(input.thinkingLevels) ||
      input.thinkingLevels.some((level) => !LEVELS.includes(level))
    )
      throw new Error('思考等级无效。')
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
