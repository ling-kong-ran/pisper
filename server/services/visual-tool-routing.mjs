const VISUAL_CONTINUATION_PATTERNS = [
  /(?:再|重新|重试|继续)(?:帮我)?(?:生成|画|绘制|制作|创建|做)(?:一下|一遍|一次|一张|一个)?(?:\s|[，。！？,.!?]|$)/i,
  /(?:再|重新)(?:来|做)(?:一下|一遍|一次|一张|一个)(?:\s|[，。！？,.!?]|$)/i,
  /\b(?:regenerate|rerender|redraw)(?:\s+(?:it|that|this))?(?:\s+again)?\b/i,
  /\b(?:generate|create|render|draw|make)\s+(?:it|that|this)\s+again\b/i,
  /\btry\s+(?:it|that|this|the generation)\s+again\b/i,
]

const VISUAL_GENERATION_PATTERNS = [
  /(?:生成|画|绘制|制作|创建|做)(?:一张|一个|一幅|一段|个)?[^。！？\n]{0,24}(?:图片|图像|插画|海报|照片|封面|壁纸|视频|动画|短片)/i,
  /(?:图片|图像|插画|海报|照片|封面|壁纸|视频|动画|短片)[^。！？\n]{0,16}(?:生成|制作|创建|画|绘制)/i,
  /(?:编辑|修改|改一下|重绘|扩图|抠图|换背景|去背景)[^。！？\n]{0,24}(?:图片|图像|照片|海报|封面|壁纸)/i,
  /(?:图片|图像|照片|海报|封面|壁纸)[^。！？\n]{0,20}(?:编辑|修改|重绘|扩图|抠图|换背景|去背景)/i,
  /\b(?:generate|create|make|draw|render)\b[^.?!\n]{0,32}\b(?:image|picture|illustration|poster|photo|video|animation|clip)\b/i,
  /\b(?:edit|modify|retouch|inpaint|outpaint|remove|replace)\b[^.?!\n]{0,32}\b(?:image|picture|illustration|poster|photo|background)\b/i,
]

export function isVisualGenerationRequest(message) {
  const value = String(message || '').trim()
  return value.length > 0 && VISUAL_GENERATION_PATTERNS.some((pattern) => pattern.test(value))
}

export function isVisualContinuationRequest(message) {
  const value = String(message || '').trim()
  return value.length > 0 && VISUAL_CONTINUATION_PATTERNS.some((pattern) => pattern.test(value))
}

export function forceToolChoice(payload, toolName) {
  if (!payload || typeof payload !== 'object') return payload
  const next = { ...payload }
  const tools = Array.isArray(payload.tools) ? payload.tools : []
  if (Array.isArray(payload.input) && tools.some((tool) => tool?.type === 'function' && tool?.name === toolName)) {
    next.tool_choice = { type: 'function', name: toolName }
    return next
  }
  if (Array.isArray(payload.messages)) {
    if (tools.some((tool) => tool?.type === 'function' && tool?.function?.name === toolName)) {
      next.tool_choice = { type: 'function', function: { name: toolName } }
      return next
    }
    if (tools.some((tool) => tool?.name === toolName && tool?.input_schema)) {
      next.tool_choice = { type: 'tool', name: toolName }
      return next
    }
  }
  if (Array.isArray(payload.contents)) {
    next.toolConfig = {
      ...(payload.toolConfig || {}),
      functionCallingConfig: { mode: 'ANY', allowedFunctionNames: [toolName] },
    }
    return next
  }
  return next
}

export function forceNextToolCall(agent, toolName) {
  const original = agent.onPayload
  let pending = true
  agent.onPayload = async (payload, model) => {
    const replaced = await original?.(payload, model)
    if (!pending) return replaced
    pending = false
    return forceToolChoice(replaced ?? payload, toolName)
  }
  return () => {
    agent.onPayload = original
  }
}
