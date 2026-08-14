export const CHAT_DOCK_LAYOUT_VERSION = 1
export const SESSION_OPEN_DISPOSITIONS = Object.freeze([
  'open',
  'left',
  'right',
  'above',
  'below',
] as const)
export type SessionOpenDisposition = (typeof SESSION_OPEN_DISPOSITIONS)[number]
export type SessionSplitDisposition = Exclude<SessionOpenDisposition, 'open'>
const SESSION_PANEL_PREFIX = 'session:'

type DockPanelLike = string | { id?: string; params?: { sessionId?: string } } | null | undefined
type DockLayout = SerializedDockview
export type DockLayoutEnvelope = {
  version: typeof CHAT_DOCK_LAYOUT_VERSION
  engine: 'dockview'
  activePanelId: string
  layout: DockLayout
}
export type SessionOpenRequest = {
  sessionId: string
  disposition: SessionOpenDisposition
  targetEntryId?: string
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function panelIdForSession(sessionId: string) {
  return sessionId ? `${SESSION_PANEL_PREFIX}${sessionId}` : ''
}

export function sessionIdFromPanel(panel: DockPanelLike) {
  const fromParams = typeof panel === 'object' && panel ? panel.params?.sessionId : undefined
  if (typeof fromParams === 'string' && fromParams) return fromParams
  const panelId = typeof panel === 'string' ? panel : panel?.id
  return typeof panelId === 'string' && panelId.startsWith(SESSION_PANEL_PREFIX)
    ? panelId.slice(SESSION_PANEL_PREFIX.length)
    : ''
}

export function createDockLayoutEnvelope(
  layout: DockLayout,
  activePanelId = '',
): DockLayoutEnvelope {
  return {
    version: CHAT_DOCK_LAYOUT_VERSION,
    engine: 'dockview',
    activePanelId: typeof activePanelId === 'string' ? activePanelId : '',
    layout,
  }
}

export function parseDockLayoutEnvelope(raw: unknown): DockLayoutEnvelope | null {
  if (!raw) return null
  try {
    const value: unknown = typeof raw === 'string' ? JSON.parse(raw) : raw
    if (
      !isRecord(value) ||
      value.version !== CHAT_DOCK_LAYOUT_VERSION ||
      value.engine !== 'dockview'
    )
      return null
    if (!isRecord(value.layout) || !isRecord(value.layout.grid) || !isRecord(value.layout.panels))
      return null
    return {
      version: CHAT_DOCK_LAYOUT_VERSION,
      engine: 'dockview',
      activePanelId: typeof value.activePanelId === 'string' ? value.activePanelId : '',
      layout: value.layout as unknown as DockLayout,
    }
  } catch {
    return null
  }
}

export function dockPositionForDisposition(
  disposition: SessionSplitDisposition,
): 'left' | 'right' | 'top' | 'bottom' {
  if (disposition === 'above') return 'top'
  if (disposition === 'below') return 'bottom'
  return disposition
}

export function createSessionOpenRequest(
  sessionId: string,
  disposition: string = 'open',
  targetEntryId = '',
): SessionOpenRequest | null {
  if (!sessionId || !SESSION_OPEN_DISPOSITIONS.includes(disposition as SessionOpenDisposition))
    return null
  return {
    sessionId,
    disposition: disposition as SessionOpenDisposition,
    ...(targetEntryId ? { targetEntryId } : {}),
  }
}

export function parseSessionOpenRequest(raw: unknown): SessionOpenRequest | null {
  if (!raw) return null
  try {
    const value: unknown = typeof raw === 'string' ? JSON.parse(raw) : raw
    if (!isRecord(value)) return null
    const sessionId = typeof value.sessionId === 'string' ? value.sessionId : ''
    const disposition = typeof value.disposition === 'string' ? value.disposition : 'open'
    const targetEntryId = typeof value.targetEntryId === 'string' ? value.targetEntryId : ''
    return createSessionOpenRequest(sessionId, disposition, targetEntryId)
  } catch {
    return null
  }
}

export function initialDockSessionIds({
  activeSessionId = '',
  legacyTiledSessionIds = [],
  validSessionIds = [],
}: {
  activeSessionId?: string
  legacyTiledSessionIds?: string[]
  validSessionIds?: string[]
} = {}): string[] {
  const valid = new Set(validSessionIds)
  const result: string[] = []
  const add = (id: string | undefined) => {
    if (!id) return
    if (valid.has(id) && !result.includes(id)) result.push(id)
  }
  add(activeSessionId)
  for (const id of legacyTiledSessionIds) add(id)
  if (!result.length) add(validSessionIds[0])
  return result
}
import type { SerializedDockview } from 'dockview-react'
