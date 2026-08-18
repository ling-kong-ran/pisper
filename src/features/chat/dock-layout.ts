// Dock 布局协议：多会话分屏（dockview）的布局序列化/反序列化，
// 以及会话面板 id 与会话 id 的互转、打开位置的语义映射。
// 布局 envelope 带版本号与引擎标识，解析失败时安全返回 null 回退默认。
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

// 类型守卫：确认值是普通对象（非数组）。
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

// 会话面板 id：session:<id> 前缀，用于 Dock 面板与会话的映射。
export function panelIdForSession(sessionId: string) {
  return sessionId ? `${SESSION_PANEL_PREFIX}${sessionId}` : ''
}

// 从面板（对象/字符串/id）反解会话 id：优先面板 params，再解析 id 前缀。
export function sessionIdFromPanel(panel: DockPanelLike) {
  const fromParams = typeof panel === 'object' && panel ? panel.params?.sessionId : undefined
  if (typeof fromParams === 'string' && fromParams) return fromParams
  const panelId = typeof panel === 'string' ? panel : panel?.id
  return typeof panelId === 'string' && panelId.startsWith(SESSION_PANEL_PREFIX)
    ? panelId.slice(SESSION_PANEL_PREFIX.length)
    : ''
}

// 构造布局 envelope（版本 + 引擎 + 当前面板 + 序列化布局）。
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

// 解析布局 envelope：版本/引擎/结构任一不匹配即返回 null（回退默认布局），
// 保证旧版本或损坏的持久化布局不会让 Dock 崩溃。
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

// 打开位置语义映射：above/below 映射为 dockview 的 top/bottom 分割。
export function dockPositionForDisposition(
  disposition: SessionSplitDisposition,
): 'left' | 'right' | 'top' | 'bottom' {
  if (disposition === 'above') return 'top'
  if (disposition === 'below') return 'bottom'
  return disposition
}

// 构造会话打开请求：sessionId 为空或位置非法返回 null；
// 有 targetEntryId 时附带定位目标。
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

// 解析会话打开请求（容忍缺字段，默认 open / 空 target）。
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

// 初始平铺会话集合：优先活动会话，再补旧版平铺列表，全部限定在合法会话内，
// 全空时取列表第一个会话，保证 Dock 启动至少有一个面板。
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
