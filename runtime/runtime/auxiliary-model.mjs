// 辅助模型：起会话标题、压缩上下文、提取与整理记忆这类后台杂活改用设置里指定的模型（通常更便宜、更快），
// 不占用当前会话的主模型。没设置或用不了（被删除、提供方停用、缺少认证）时各自退回原来的模型，
// 行为与未设置时一致。设置保存在 pisper.json 的 auxiliaryModel，值为 "provider/modelId"。

const MAX_MODEL_REF_CHARS = 300
const TITLE_TIMEOUT_MS = 20_000
const TITLE_SOURCE_CHARS = 2_000

const TITLE_SYSTEM_PROMPT = [
  'You write a short title for a conversation with a coding agent, based on the first user message.',
  'Name the task itself, in the language of the message. Chinese titles: at most 16 characters; other languages: at most 8 words.',
  'Output only the title on one line: no quotes, no trailing punctuation, no explanations.',
].join('\n')

// 设置里的模型引用：只认 "provider/modelId"，其余一律视为未设置。
export function normalizeAuxiliaryModelRef(value) {
  const ref = typeof value === 'string' ? value.trim() : ''
  const slash = ref.indexOf('/')
  return slash > 0 && slash < ref.length - 1 && ref.length <= MAX_MODEL_REF_CHARS ? ref : ''
}

function textContent(content) {
  if (typeof content === 'string') return content
  return Array.isArray(content)
    ? content
        .filter((part) => part?.type === 'text')
        .map((part) => part.text || '')
        .join('')
    : ''
}

/**
 * 用辅助模型为首条消息起标题。返回模型原文（由调用方按会话标题规则清洗）与用量；
 * 失败、超时或空回复时 text 为空字符串，不抛出，避免影响正在进行的这一轮。
 */
export async function generateSessionTitle({ modelRuntime, model, message, signal }) {
  const source = String(message || '')
    .trim()
    .slice(0, TITLE_SOURCE_CHARS)
  if (!modelRuntime?.completeSimple || !model || !source) return { text: '' }
  try {
    const result = await modelRuntime.completeSimple(
      model,
      {
        systemPrompt: TITLE_SYSTEM_PROMPT,
        messages: [{ role: 'user', content: source, timestamp: Date.now() }],
      },
      {
        // 推理与正文共享输出上限：推理模型留足余量，否则只产生思考、没有标题。
        ...(model.reasoning ? { reasoning: 'low' } : { temperature: 0.2 }),
        maxTokens: Math.min(
          model.reasoning ? 4096 : 64,
          Number(model.maxTokens) > 0 ? Number(model.maxTokens) : Infinity,
        ),
        signal,
      },
    )
    if (signal?.aborted || result?.errorMessage || result?.stopReason === 'error') {
      return { text: '' }
    }
    return { text: textContent(result?.content).trim(), usage: result?.usage }
  } catch {
    return { text: '' }
  }
}

// 后台标题任务的所有者：每个任务带超时，关闭 Runtime 时统一取消，不留悬挂的请求。
export class AuxiliaryTitleTasks {
  constructor({ timeoutMs = TITLE_TIMEOUT_MS } = {}) {
    this.timeoutMs = timeoutMs
    this.controllers = new Set()
    this.closed = false
  }

  run(work) {
    if (this.closed) return Promise.resolve()
    const controller = new AbortController()
    const timer = setTimeout(() => controller.abort(), this.timeoutMs)
    timer.unref?.()
    this.controllers.add(controller)
    return Promise.resolve()
      .then(() => work(controller.signal))
      .catch(() => {})
      .finally(() => {
        clearTimeout(timer)
        this.controllers.delete(controller)
      })
  }

  dispose() {
    this.closed = true
    for (const controller of this.controllers) controller.abort()
    this.controllers.clear()
  }
}
