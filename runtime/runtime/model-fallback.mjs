// 备用模型：主模型在同一模型的自动重试用尽后仍不可用（过载、限流、5xx、断流），
// 或直接报额度/余额耗尽（Pi 对这类错误不重试）时，按设置里的顺序切到下一个备用模型，
// 并从失败处接着跑，而不是从头重跑这一轮（已经执行过的工具不会再执行一遍）。
// 挂在 Pi 自己的重试路径上：失败的那次回复按 Pi 的方式从模型上下文里剔除，再 continue。
// 鉴权失败、上下文溢出、用户中止和其它请求错误不切换，照常报错。
const MODEL_FALLBACK_PATCH = Symbol('pisper.model-fallback')

export const MAX_FALLBACK_MODELS = 2

const QUOTA_ERROR_PATTERN =
  /insufficient_quota|quota\s+exceeded|out\s+of\s+budget|billing|usage\s+limit|available\s+balance|insufficient\s+(?:balance|credit)|额度|余额不足/i
const AUTHENTICATION_ERROR_PATTERN =
  /\b(?:401|403|unauthori[sz]ed|forbidden|invalid\s+(?:api\s*key|token|credential)|authentication\s+(?:failed|required|error))\b/i

export function isQuotaError(message) {
  return (
    message?.stopReason === 'error' && QUOTA_ERROR_PATTERN.test(String(message.errorMessage || ''))
  )
}

// 设置里保存的是 "provider/modelId" 列表：去空、去重、限长，非法项直接丢弃。
export function normalizeFallbackModels(value) {
  const refs = []
  for (const item of Array.isArray(value) ? value : []) {
    if (typeof item !== 'string') continue
    const ref = item.trim()
    const slash = ref.indexOf('/')
    if (slash <= 0 || slash === ref.length - 1 || ref.length > 300) continue
    if (!refs.includes(ref)) refs.push(ref)
    if (refs.length >= MAX_FALLBACK_MODELS) break
  }
  return refs
}

export function modelRef(model) {
  return model ? `${model.provider}/${model.id}` : ''
}

/**
 * @param {any} session Pi AgentSession
 * @param {{
 *   resolveCandidates: () => Promise<any[]>,
 *   switchModel?: (model: any) => Promise<void>,
 *   onFallback?: (event: { from: any, to: any, errorMessage: string }) => void | Promise<void>,
 * }} options
 */
export function installModelFallback(
  session,
  { resolveCandidates, switchModel = (model) => session.setModel(model), onFallback },
) {
  if (
    !session ||
    session[MODEL_FALLBACK_PATCH] ||
    typeof session._isRetryableError !== 'function' ||
    typeof session._prepareRetry !== 'function' ||
    typeof session._omitRecoveryAttempt !== 'function'
  )
    return session
  // 每次 prompt 开始时取一次备用列表；同一次运行里每个备用模型只试一次，避免几个模型互相来回切。
  let candidates = []
  const tried = new Set()
  const prompt = session.prompt.bind(session)
  session.prompt = async (...args) => {
    tried.clear()
    tried.add(modelRef(session.model))
    candidates = await resolveCandidates().catch(() => [])
    return prompt(...args)
  }
  const hasCandidate = () => candidates.some((model) => !tried.has(modelRef(model)))

  const isRetryableError = session._isRetryableError.bind(session)
  session._isRetryableError = (message) =>
    isRetryableError(message) ||
    (isQuotaError(message) &&
      !AUTHENTICATION_ERROR_PATTERN.test(String(message.errorMessage || '')) &&
      hasCandidate())

  const prepareRetry = session._prepareRetry.bind(session)
  session._prepareRetry = async (message) => {
    // 额度耗尽时同一模型重试也没用，直接切；其余可重试错误先按 Pi 的策略重试同一模型。
    if (!isQuotaError(message) && isRetryableError(message)) {
      if (await prepareRetry(message)) return true
    }
    if (session._agentRunAbortRequested) return false
    for (const model of candidates) {
      const ref = modelRef(model)
      if (tried.has(ref)) continue
      tried.add(ref)
      const from = session.model
      try {
        await switchModel(model)
      } catch {
        // 备用模型缺凭据等：试下一个。
        continue
      }
      session._omitRecoveryAttempt(message)
      // 新模型从头计算重试次数。
      session._retryAttempt = 0
      try {
        await onFallback?.({ from, to: model, errorMessage: String(message.errorMessage || '') })
      } catch {
        // 通知失败不影响续跑。
      }
      return true
    }
    return false
  }
  session[MODEL_FALLBACK_PATCH] = true
  return session
}
