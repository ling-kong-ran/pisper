import { compact } from '@earendil-works/pi-coding-agent'

export const COMPACTION_TARGET_RESERVE_RATIO = 0.2
export const COMPACTION_MAX_RESERVE_TOKENS = 65_536
export const COMPACTION_SUMMARY_RESERVE_TOKENS = 16_384

function tokenCount(value, fallback = 0) {
  const number = Math.floor(Number(value) || 0)
  return number > 0 ? number : fallback
}

export function effectiveCompactionSettings(settings = {}, contextWindow = 0) {
  const reserveTokens = tokenCount(settings.reserveTokens, COMPACTION_SUMMARY_RESERVE_TOKENS)
  const windowTokens = tokenCount(contextWindow)
  const adaptiveReserve = windowTokens
    ? Math.min(COMPACTION_MAX_RESERVE_TOKENS, Math.floor(windowTokens * COMPACTION_TARGET_RESERVE_RATIO))
    : 0
  return {
    ...settings,
    enabled: settings.enabled !== false,
    reserveTokens: Math.max(reserveTokens, adaptiveReserve),
    keepRecentTokens: tokenCount(settings.keepRecentTokens, 20_000),
  }
}

export function createCompactionSettingsManager(settingsManager, getContextWindow = () => 0) {
  if (!settingsManager) return settingsManager
  return new Proxy(settingsManager, {
    get(target, property) {
      if (property === 'getCompactionSettings') {
        return () => effectiveCompactionSettings(target.getCompactionSettings(), getContextWindow())
      }
      const value = Reflect.get(target, property, target)
      return typeof value === 'function' ? value.bind(target) : value
    },
  })
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
