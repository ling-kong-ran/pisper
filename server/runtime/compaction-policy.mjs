import { compact } from './pi-coding-agent.mjs'

export const DEFAULT_COMPACTION_THRESHOLD_PERCENT = 80
export const MIN_COMPACTION_THRESHOLD_PERCENT = 50
export const MAX_COMPACTION_THRESHOLD_PERCENT = 95
export const COMPACTION_SUMMARY_RESERVE_TOKENS = 16_384

const PISPER_TURN_COMPACTION_PATCH = Symbol('pisper.turn-compaction-patch')

function tokenCount(value, fallback = 0) {
  const number = Math.floor(Number(value) || 0)
  return number > 0 ? number : fallback
}

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
