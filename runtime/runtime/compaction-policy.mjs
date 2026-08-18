// 压缩策略：包装 Pi 引擎的压缩设置与触发逻辑，加入 Pisper 侧的阈值/预留 token 策略，
// 并确保压缩发生在回合边界（工具结果落地之后），避免上下文被中途改写。
import { compact } from './pi-coding-agent.mjs'

export const DEFAULT_COMPACTION_THRESHOLD_PERCENT = 80
export const MIN_COMPACTION_THRESHOLD_PERCENT = 50
export const MAX_COMPACTION_THRESHOLD_PERCENT = 95
// 摘要响应的预留 token：压缩时不能把上下文窗口全部占满，否则摘要无法生成。
export const COMPACTION_SUMMARY_RESERVE_TOKENS = 16_384

const PISPER_TURN_COMPACTION_PATCH = Symbol('pisper.turn-compaction-patch')

function tokenCount(value, fallback = 0) {
  const number = Math.floor(Number(value) || 0)
  return number > 0 ? number : fallback
}

// 把阈值限制到 [MIN, MAX] 区间，非法输入回退到默认值。
export function normalizeCompactionThresholdPercent(
  value,
  fallback = DEFAULT_COMPACTION_THRESHOLD_PERCENT,
) {
  const number = Number(value)
  if (!Number.isFinite(number)) return fallback
  return Math.min(
    MAX_COMPACTION_THRESHOLD_PERCENT,
    Math.max(MIN_COMPACTION_THRESHOLD_PERCENT, Math.round(number)),
  )
}

// 计算有效压缩设置：由阈值百分比反推预留 token（窗口余量），
// 未显式配置时兜底使用默认预留/保留最近 token 数。
export function effectiveCompactionSettings(
  settings = {},
  contextWindow = 0,
  thresholdPercent = DEFAULT_COMPACTION_THRESHOLD_PERCENT,
) {
  const windowTokens = tokenCount(contextWindow)
  const normalizedThreshold = normalizeCompactionThresholdPercent(thresholdPercent)
  const thresholdReserve = windowTokens
    ? Math.max(1, windowTokens - Math.floor((windowTokens * normalizedThreshold) / 100))
    : 0
  return {
    ...settings,
    enabled: settings.enabled !== false,
    reserveTokens:
      thresholdReserve || tokenCount(settings.reserveTokens, COMPACTION_SUMMARY_RESERVE_TOKENS),
    keepRecentTokens: tokenCount(settings.keepRecentTokens, 20_000),
  }
}

// 用 Proxy 覆盖 SettingsManager 的 getCompactionSettings：
// 让会话的压缩设置动态反映当前模型上下文窗口与全局阈值，无需每次手动传入。
export function createCompactionSettingsManager(
  settingsManager,
  getContextWindow = () => 0,
  getThresholdPercent = () => DEFAULT_COMPACTION_THRESHOLD_PERCENT,
) {
  if (!settingsManager) return settingsManager
  return new Proxy(settingsManager, {
    get(target, property) {
      if (property === 'getCompactionSettings') {
        return () =>
          effectiveCompactionSettings(
            target.getCompactionSettings(),
            getContextWindow(),
            getThresholdPercent(),
          )
      }
      const value = Reflect.get(target, property, target)
      return typeof value === 'function' ? value.bind(target) : value
    },
  })
}

// 回合边界压缩补丁：在 prepareNextTurnWithContext（工具结果后的下一轮准备）时
// 先检查压缩；若发生了压缩（叶子节点变化），用压缩后的上下文替换本轮快照，
// 避免下一轮基于压缩前的旧上下文继续。
export function installTurnBoundaryCompaction(session) {
  if (
    !session?.agent ||
    session[PISPER_TURN_COMPACTION_PATCH] ||
    typeof session._checkCompaction !== 'function' ||
    typeof session.sessionManager?.buildSessionContext !== 'function'
  )
    return session

  const previousPrepareNextTurnWithContext = session.agent.prepareNextTurnWithContext
  session.agent.prepareNextTurnWithContext = async (turn, signal) => {
    const previousSnapshot = await previousPrepareNextTurnWithContext?.(turn, signal)
    if (!turn.toolResults?.length) return previousSnapshot

    const previousContext = previousSnapshot?.context ?? turn.context
    const leafBefore = session.sessionManager.getLeafId?.()
    await session._checkCompaction(turn.message)
    const leafAfter = session.sessionManager.getLeafId?.()
    if (leafBefore === leafAfter) return previousSnapshot

    return {
      ...previousSnapshot,
      context: {
        ...previousContext,
        messages: session.sessionManager.buildSessionContext().messages,
      },
    }
  }
  session[PISPER_TURN_COMPACTION_PATCH] = true
  return session
}

// 压缩扩展：让 Pi 引擎在 session_before_compact 钩子中使用 Pisper 的压缩实现，
// 并收紧预留 token——更早的压缩不应同时放大摘要响应的可用空间。
export function pisperCompactionExtension(pi, { compactSession = compact } = {}) {
  pi.on('session_before_compact', async (event, context) => {
    const model = context.model
    if (!model) return undefined
    const auth = await context.modelRegistry.getApiKeyAndHeaders(model)
    if (!auth?.ok) return undefined
    const preparation = {
      ...event.preparation,
      settings: {
        ...event.preparation.settings,
        // Earlier compaction should not also enlarge the possible summary response.
        reserveTokens: Math.min(
          tokenCount(event.preparation.settings?.reserveTokens, COMPACTION_SUMMARY_RESERVE_TOKENS),
          COMPACTION_SUMMARY_RESERVE_TOKENS,
        ),
      },
    }
    const result = await compactSession(
      preparation,
      model,
      auth.apiKey,
      auth.headers,
      event.customInstructions,
      event.signal,
      'off',
      undefined,
      auth.env,
    )
    return { compaction: result }
  })
}
