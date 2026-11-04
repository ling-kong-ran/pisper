// 会话状态的“提醒时刻”：需要你（等批准或回答）、跑完、失败。从看板卡片前后两次快照的差异里推出来，
// 提示音等提醒按它响。首次载入只记快照、不提醒，免得一打开就把既有事项全响一遍。
import { sidebarReasons, type SidebarSourceItem } from './sidebar-model'

export type SessionAlertKind = 'waiting' | 'finished' | 'failed'
export type SessionAlert = { kind: SessionAlertKind; id: string; title: string }
export type SessionAlertSnapshot = ReadonlyMap<string, SessionAlertState>
type SessionAlertState = 'waiting' | 'failed' | 'running' | 'done' | 'other'

function alertState(item: SidebarSourceItem): SessionAlertState {
  const reasons = sidebarReasons(item)
  if (reasons.some((reason) => reason.kind === 'failed')) return 'failed'
  if (reasons.some((reason) => reason.kind === 'approval' || reason.kind === 'question'))
    return 'waiting'
  if (item.lane === 'running') return 'running'
  if (item.lane === 'done') return 'done'
  return 'other'
}

export function sessionAlerts(
  previous: SessionAlertSnapshot | null,
  items: readonly SidebarSourceItem[],
): { snapshot: SessionAlertSnapshot; alerts: SessionAlert[] } {
  const snapshot = new Map<string, SessionAlertState>()
  const alerts: SessionAlert[] = []
  for (const item of items) {
    if (!item || typeof item.id !== 'string' || !item.id || snapshot.has(item.id)) continue
    const state = alertState(item)
    snapshot.set(item.id, state)
    if (!previous) continue
    const before = previous.get(item.id)
    if (state === before) continue
    // 新出现就在等你的会话也算；失败与跑完只认“之前在跑或在等”的会话，
    // 免得旧会话因为时间窗或筛选变化重新进入列表时误报。
    if (state === 'waiting') alerts.push({ kind: 'waiting', id: item.id, title: item.title })
    else if (state === 'failed' && (before === 'running' || before === 'waiting'))
      alerts.push({ kind: 'failed', id: item.id, title: item.title })
    else if (state === 'done' && (before === 'running' || before === 'waiting'))
      alerts.push({ kind: 'finished', id: item.id, title: item.title })
  }
  return { snapshot, alerts }
}
