const CHINESE_VISUAL_NOUN = '(?:图片|图像|插画|海报|照片|封面|壁纸|头像|图标|标志|横幅|设计图|效果图|概念图|示意图|草图|原型图|线框图|视觉稿|UI\\s*稿|界面稿|Logo|logo|视频|动画|短片)'
const CHINESE_VISUAL_ACTION = '(?:生成|画|绘制|制作|创建|做|设计|渲染|出|来|整|弄)'
const ENGLISH_VISUAL_NOUN = '(?:image|picture|illustration|poster|photo|cover|wallpaper|avatar|icon|logo|banner|mockup|wireframe|concept\\s+art|video|animation|clip)'
const ENGLISH_VISUAL_ACTION = '(?:generate|create|make|draw|render|design|produce|mock\\s+up)'
const VISUAL_ACTION_TOKEN = /(?:生成|画|绘制|制作|创建|做|设计|渲染|出|来|整|弄|编辑|修改|重绘|扩图|抠图|换背景|去背景|generate|create|make|draw|render|design|produce|mock\s+up|edit|modify|retouch|inpaint|outpaint|remove|replace)/gi

const VISUAL_CONTINUATION_PATTERNS = [
  /(?:再|重新|重试|继续)(?:帮我)?(?:生成|生图|画|绘制|制作|创建|做|设计|渲染|出图)(?:一下|一遍|一次|一张|一个)?(?:\s|[，。！？,.!?]|$)/i,
  /(?:再|重新)(?:来|做|弄|整|出)(?:一下|一遍|一次|一张|一个|张图|个图)?(?:\s|[，。！？,.!?]|$)/i,
  /(?:生图|出图|生成图片)[^。！？\n]{0,12}(?:失败|没成功|没生成|没有生成|出错)[^。！？\n]{0,12}(?:重试|再试|重来|重新)/i,
  /\b(?:regenerate|rerender|redraw)(?:\s+(?:it|that|this))?(?:\s+again)?\b/i,
  /\b(?:generate|create|render|draw|make|design)\s+(?:it|that|this)\s+again\b/i,
  /\btry\s+(?:it|that|this|the generation)\s+again\b/i,
]

const VISUAL_GENERATION_PATTERNS = [
  /\bgenerate_visual\b/i,
  new RegExp(`${CHINESE_VISUAL_ACTION}(?:一张|一个|一幅|一段|一份|张|个|份)?[^。！？\\n]{0,24}${CHINESE_VISUAL_NOUN}`, 'i'),
  new RegExp(`${CHINESE_VISUAL_NOUN}[^。！？\\n]{0,16}${CHINESE_VISUAL_ACTION}`, 'i'),
  new RegExp(`(?:编辑|修改|改一下|重绘|扩图|抠图|换背景|去背景)[^。！？\\n]{0,24}${CHINESE_VISUAL_NOUN}`, 'i'),
  new RegExp(`${CHINESE_VISUAL_NOUN}[^。！？\\n]{0,20}(?:编辑|修改|重绘|扩图|抠图|换背景|去背景)`, 'i'),
  new RegExp(`\\b${ENGLISH_VISUAL_ACTION}\\b[^.?!\\n]{0,32}\\b${ENGLISH_VISUAL_NOUN}\\b`, 'i'),
  new RegExp(`\\b(?:edit|modify|retouch|inpaint|outpaint|remove|replace)\\b[^.?!\\n]{0,32}\\b${ENGLISH_VISUAL_NOUN}\\b`, 'i'),
]

const VISUAL_SUCCESS_PATTERNS = [
  new RegExp(`(?:已|已经|刚刚|现已|成功)(?:为你|给你)?(?:重新|再次|再)?${CHINESE_VISUAL_ACTION}[^。！？\\n]{0,16}${CHINESE_VISUAL_NOUN}`, 'i'),
  new RegExp(`(?:重新|再次|再)${CHINESE_VISUAL_ACTION}(?:完成|成功|好了|完毕|了)?[^。！？\\n]{0,12}${CHINESE_VISUAL_NOUN}`, 'i'),
  new RegExp(`${CHINESE_VISUAL_NOUN}[^。！？\\n]{0,12}(?:已|已经|刚刚|现已|成功)(?:重新|再次|再)?(?:生成|绘制|制作|创建|渲染|画好|做好)(?:完成|成功|好了|完毕|了)?`, 'i'),
  new RegExp(`\\b(?:i(?:'ve| have)?\\s+)?(?:successfully\\s+)?(?:generated|created|rendered|regenerated|redrawn|produced)\\b[^.?!\\n]{0,32}\\b${ENGLISH_VISUAL_NOUN}\\b`, 'i'),
]

function isNegatedAt(text, index) {
  const segment = text.slice(Math.max(0, index - 28), index).split(/[，。！？,.!?;；]/).at(-1) || ''
  return /(?:不要|无需|不需要|禁止|别|不得|请勿|没有|并未|尚未|未能|无法|不能|失败|don't|do not|never|without|didn't|did not|haven't|hasn't|failed to)(?:\s|\S){0,12}$/i.test(segment)
}

function hasPositiveVisualMatch(value, patterns) {
  for (const pattern of patterns) {
    const globalPattern = new RegExp(pattern.source, pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`)
    for (const match of value.matchAll(globalPattern)) {
      const actionMatches = [...match[0].matchAll(VISUAL_ACTION_TOKEN)]
      const actionIndex = (match.index || 0) + (actionMatches.at(-1)?.index || 0)
      if (!isNegatedAt(value, actionIndex)) return true
    }
  }
  return false
}

export function isVisualGenerationRequest(message) {
  const value = String(message || '').trim()
  return value.length > 0 && hasPositiveVisualMatch(value, VISUAL_GENERATION_PATTERNS)
}

export function isVisualSuccessClaim(message) {
  const value = String(message || '').trim()
  return value.length > 0 && hasPositiveVisualMatch(value, VISUAL_SUCCESS_PATTERNS)
}

export function visualClaimValidationError({ assistantText = '', messageCallsVisualTool = false, tools = [] } = {}) {
  if (!isVisualSuccessClaim(assistantText) || messageCallsVisualTool) return ''
  const succeeded = tools.some((tool) => tool?.name === 'generate_visual' && tool?.status === 'done')
  return succeeded ? '' : '本轮没有成功执行 generate_visual，因此没有生成或重新生成任何视觉文件。'
}

function messageCallsVisualTool(message) {
  return Array.isArray(message?.content) && message.content.some((part) => part?.type === 'toolCall' && part?.name === 'generate_visual')
}

function replaceAssistantWithVisualError(message, error) {
  message.content = [{ type: 'text', text: error }]
  message.stopReason = 'error'
  message.errorMessage = error
  return error
}

export function enforceRequiredVisualToolCall(message, tools = [], required = false) {
  if (!required || message?.role !== 'assistant' || messageCallsVisualTool(message)) return ''
  if (tools.some((tool) => tool?.name === 'generate_visual')) return ''
  return replaceAssistantWithVisualError(message, '该视觉请求未执行 generate_visual，因此没有生成任何视觉文件。')
}

export function enforceVisualClaimEvidence(message, tools = []) {
  if (message?.role !== 'assistant') return ''
  const content = Array.isArray(message.content) ? message.content : []
  const assistantText = content.filter((part) => part?.type === 'text').map((part) => part.text || '').join('\n')
  const error = visualClaimValidationError({ assistantText, messageCallsVisualTool: messageCallsVisualTool(message), tools })
  return error ? replaceAssistantWithVisualError(message, error) : ''
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
