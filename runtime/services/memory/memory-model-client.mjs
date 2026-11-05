// 记忆后台任务调模型的唯一入口：只走辅助模型（语义扩展量小，允许退回主模型），计入用量账本，
// 有每日 token 上限，超出后任务顺延到第二天。SDK 错误可能含请求原文，只向外返回稳定诊断码。
export const DEFAULT_MEMORY_DAILY_TOKEN_BUDGET = 200_000

function textFromContent(content) {
  if (typeof content === 'string') return content
  return Array.isArray(content)
    ? content
        .filter((part) => part?.type === 'text')
        .map((part) => part.text || '')
        .join('')
    : ''
}

function localDay(now) {
  const date = new Date(now)
  const pad = (part) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

function totalTokens(usage) {
  const value = Number(usage?.totalTokens ?? usage?.total ?? 0)
  if (value > 0) return value
  return Number(usage?.input || 0) + Number(usage?.output || 0)
}

export function createMemoryModelClient({
  getModelRuntime,
  getAuxiliaryModel,
  getDefaultModel,
  recordUsage,
  dailyBudget = DEFAULT_MEMORY_DAILY_TOKEN_BUDGET,
  now = () => Date.now(),
}) {
  const spent = { day: '', tokens: 0 }
  const spentToday = () => {
    const day = localDay(now())
    if (spent.day !== day) {
      spent.day = day
      spent.tokens = 0
    }
    return spent
  }

  async function resolve(allowMain) {
    const auxiliary = await getAuxiliaryModel?.()
    if (auxiliary) return auxiliary
    return allowMain ? (await getDefaultModel?.()) || null : null
  }

  return {
    // 没配辅助模型时，批量抽取、对账和整理都不跑：不能悄悄花主模型的钱。
    async available({ allowMain = false } = {}) {
      if (!getModelRuntime?.()) return false
      if (spentToday().tokens >= dailyBudget) return false
      return Boolean(await resolve(allowMain))
    },

    /**
     * @param {{ purpose: string, system: string, user: string, maxTokens?: number,
     *   signal?: AbortSignal, allowMain?: boolean }} request
     * @returns {Promise<{ text: string, model: string } | { errorCode: string }>}
     */
    async complete({ purpose, system, user, maxTokens = 2048, signal, allowMain = false }) {
      const modelRuntime = getModelRuntime?.()
      if (!modelRuntime) return { errorCode: 'model_unavailable' }
      if (spentToday().tokens >= dailyBudget) return { errorCode: 'budget_exhausted' }
      const model = await resolve(allowMain)
      if (!model) return { errorCode: 'model_unavailable' }
      if (signal?.aborted) return { errorCode: 'aborted' }
      let result
      try {
        result = await modelRuntime.completeSimple(
          model,
          {
            systemPrompt: system,
            messages: [{ role: 'user', content: user, timestamp: now() }],
          },
          {
            ...(model.reasoning ? { reasoning: 'low' } : { temperature: 0.1 }),
            // 推理与正文可能共享输出上限，太小会只产生思考（见 conversation-memory.mjs）。
            maxTokens: Math.min(
              model.reasoning ? Math.max(maxTokens, 8192) : maxTokens,
              Number(model.maxTokens) > 0 ? Number(model.maxTokens) : Infinity,
            ),
            signal,
          },
        )
      } catch {
        return { errorCode: signal?.aborted ? 'aborted' : 'model_error' }
      }
      if (result?.usage) {
        spentToday().tokens += totalTokens(result.usage)
        await Promise.resolve(
          recordUsage?.(
            localDay(result.timestamp || now()),
            `memory:${purpose}:${result.timestamp || now()}:${Math.random().toString(36).slice(2, 10)}`,
            result.usage,
          ),
        ).catch(() => {})
      }
      if (signal?.aborted || result?.stopReason === 'aborted') return { errorCode: 'aborted' }
      if (result?.errorMessage || result?.stopReason === 'error')
        return { errorCode: 'model_error' }
      if (result?.stopReason === 'length') return { errorCode: 'token_limit' }
      const text = textFromContent(result?.content)
      if (!text.trim()) return { errorCode: 'empty_response' }
      return { text, model: `${model.provider || ''}/${model.id || ''}` }
    },
  }
}

// 模型回复里取第一个 JSON 值：模型常常裹一层 ``` 或说两句话。解析不出来返回 null。
export function parseModelJson(text, opening = '{', closing = '}') {
  const source = String(text || '')
    .trim()
    .replace(/^```(?:json)?\s*/i, '')
    .replace(/\s*```$/i, '')
  const start = source.indexOf(opening)
  const end = source.lastIndexOf(closing)
  if (start < 0 || end <= start) return null
  try {
    return JSON.parse(source.slice(start, end + 1))
  } catch {
    return null
  }
}
