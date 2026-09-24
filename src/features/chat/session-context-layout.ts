export type SessionContextPreference = 'auto' | 'open' | 'closed'
export type SessionContextPresentation = 'aside' | 'sheet' | 'closed'

export function resolveSessionContextPresentation({
  availableWidth,
  mobileLayout,
  hasSession,
  preference,
}: {
  availableWidth: number
  mobileLayout: boolean
  hasSession: boolean
  preference: SessionContextPreference
}): SessionContextPresentation {
  if (!hasSession || preference === 'closed') return 'closed'
  const compact = mobileLayout || availableWidth < 800
  if (preference === 'auto' && compact) return 'closed'
  return compact ? 'sheet' : 'aside'
}
