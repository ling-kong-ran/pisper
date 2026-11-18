// 一些 Anthropic 兼容网关只在 message_delta 返回统计，Pi 却在 message_start 无条件读取 usage。
// 只补缺失的起始统计；后续累计值仍由 Pi 更新，不能把重复的结束事件当作增量重复计费。
/** @param {unknown} event */
function normalizeStartUsage(event) {
  if (!event || typeof event !== 'object' || !('type' in event)) return
  if (event.type !== 'message_start' || !('message' in event)) return
  const message = event.message
  if (!message || typeof message !== 'object' || Array.isArray(message)) return
  if (!('usage' in message) || message.usage == null) {
    Object.assign(message, { usage: { input_tokens: 0, output_tokens: 0 } })
  }
}

/**
 * @template {import('@earendil-works/pi-ai').StreamOptions} T
 * @param {T | undefined} options
 */
export function withAnthropicStreamCompatibility(options) {
  return {
    ...options,
    /** @type {NonNullable<import('@earendil-works/pi-ai').StreamOptions['onProviderStreamEvent']>} */
    onProviderStreamEvent: async (event, model) => {
      // 保留调用方的异步观察/修改钩子，补齐后再交回 Pi 消费。
      await options?.onProviderStreamEvent?.(event, model)
      normalizeStartUsage(event)
    },
  }
}
