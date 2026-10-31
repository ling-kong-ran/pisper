// 看板实时数据源（第 2 期 Runtime 的事件流 / 看板视图）的纯状态推进：不发请求、不读写 store，
// 每一帧都是不可信输入，先校验再改状态；结果里带上 store 需要执行的副作用（写目录缓存、补读目录、重连）。
// 规则：
// - snapshot 整体替换条目（同时也是 resync_required 之后“丢弃旧覆盖层”的落点）；
// - session_card 只接受修订号大于已见修订号的帧，重复与乱序帧直接丢弃；
// - resync_required 之后、新快照到达之前的增量一律忽略，flush_failed 还要求断开重连（不带 after）；
// - heartbeat / component_changed 只推进修订号，供断线后带 after 补发。
// 条目保存 Runtime 给出的原样审批；“正在提交决定的审批不再冒出来”在派生卡片时过滤（board-model）。
import type { SessionSummary } from '@/types/chat'
import {
  parseBoardCard,
  parseBoardFeedItem,
  parseBoardRevision,
  parseBoardSessionRow,
  type BoardFeedItem,
} from './board-api'

export type BoardFeedMode = 'legacy' | 'view' | 'events'

export type BoardFeedEntry = BoardFeedItem & {
  revision: number
  // 本地收到这行会话数据的时间：目录重新读取后，用它判断条目与目录谁更新（没有时按更新处理）。
  receivedAt?: number
}

export type BoardFeedState = {
  // 当前采用的数据源：legacy 为第 1 期降级路径（/api/sessions + /live），没有条目。
  mode: BoardFeedMode
  // 已经收到第一份快照或看板视图：此后看板成员与 card 以条目为准。
  ready: boolean
  // 最近一次确认的修订号，断线重连时作为 after。
  revision: number | null
  // 收到 resync_required 后等待新快照：期间的增量不可信，全部忽略。
  awaitingSnapshot: boolean
  entries: Record<string, BoardFeedEntry>
  // 最近一次收到数据的本地时间，用来推进“今天”的判断。
  updatedAt: number
}

export type BoardFeedEffects = {
  // 需要写回权威目录缓存的会话行（不含 card）。
  upserts: SessionSummary[]
  // 离开看板的会话（归档、删除或出了时间窗）：目录缓存里的状态需要补读确认。
  removed: string[]
  // 收到 resync_required：目录需要全量补读。
  resync: boolean
  // 需要立即断开并不带 after 重连（flush_failed）。
  reconnect: boolean
  // 快照被截断（超过 500 条）。
  truncated: boolean
  // 收到 catalog_changed（A9）：不在看板上的会话改名 / 归档 / 删除了，目录需要补读（不必等快照）。
  catalog: boolean
  // 状态有变化，需要写回 store。
  changed: boolean
}

export function emptyBoardFeed(mode: BoardFeedMode = 'legacy'): BoardFeedState {
  return { mode, ready: false, revision: null, awaitingSnapshot: false, entries: {}, updatedAt: 0 }
}

function noEffects(): BoardFeedEffects {
  return {
    upserts: [],
    removed: [],
    resync: false,
    reconnect: false,
    truncated: false,
    catalog: false,
    changed: false,
  }
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

/**
 * 用一份完整的看板条目（事件流快照或 view=board 结果）整体替换覆盖层。
 * revision 为 null 时（view 响应没有修订号）保留原修订号。
 */
export function replaceBoardFeed(
  feed: BoardFeedState,
  items: readonly BoardFeedItem[],
  revision: number | null,
  now = Date.now(),
): { feed: BoardFeedState; effects: BoardFeedEffects } {
  const effects = noEffects()
  const entries: Record<string, BoardFeedEntry> = {}
  const entryRevision = revision ?? feed.revision ?? 0
  for (const item of items) {
    entries[item.session.id] = { ...item, revision: entryRevision, receivedAt: now }
    effects.upserts.push(item.session)
  }
  for (const id of Object.keys(feed.entries)) if (!(id in entries)) effects.removed.push(id)
  effects.changed = true
  return {
    feed: {
      ...feed,
      ready: true,
      revision: revision ?? feed.revision,
      awaitingSnapshot: false,
      entries,
      updatedAt: now,
    },
    effects,
  }
}

/** 推进一帧事件流。未知事件与不合法的帧不改变状态。 */
export function applyBoardFeedEvent(
  feed: BoardFeedState,
  event: string,
  data: unknown,
  now = Date.now(),
): { feed: BoardFeedState; effects: BoardFeedEffects } {
  const frame = record(data)
  const revision = parseBoardRevision(frame.revision)
  const unchanged = { feed, effects: noEffects() }
  if (event === 'snapshot') {
    if (revision === null || !Array.isArray(frame.items)) return unchanged
    const items: BoardFeedItem[] = []
    const seen = new Set<string>()
    for (const value of frame.items) {
      const item = parseBoardFeedItem(value)
      if (!item || seen.has(item.session.id)) continue
      seen.add(item.session.id)
      items.push(item)
    }
    const result = replaceBoardFeed(feed, items, revision, now)
    result.effects.truncated = frame.truncated === true
    return result
  }
  if (event === 'resync_required') {
    const effects = noEffects()
    effects.resync = true
    effects.reconnect = frame.reason === 'flush_failed'
    effects.changed = !feed.awaitingSnapshot
    return { feed: { ...feed, awaitingSnapshot: true }, effects }
  }
  // 等新快照期间，其余帧都不可信。
  if (feed.awaitingSnapshot || revision === null) return unchanged
  // 重复或乱序：修订号不超过已见修订号的帧已经包含在当前状态里。
  if (feed.revision !== null && revision <= feed.revision) return unchanged
  if (event === 'heartbeat' || event === 'component_changed' || event === 'catalog_changed') {
    const effects = noEffects()
    effects.changed = true
    effects.catalog = event === 'catalog_changed'
    return { feed: { ...feed, revision, updatedAt: now }, effects }
  }
  if (event !== 'session_card') return unchanged
  const id = typeof frame.id === 'string' ? frame.id : ''
  if (!id) return unchanged
  const current = feed.entries[id]
  if (current && revision <= current.revision) return unchanged
  const effects = noEffects()
  effects.changed = true
  if (frame.removed === true) {
    const entries = { ...feed.entries }
    delete entries[id]
    if (current) effects.removed.push(id)
    return { feed: { ...feed, revision, entries, updatedAt: now }, effects }
  }
  const session = parseBoardSessionRow(frame.session)
  // 帧里的会话 id 必须与帧 id 一致，否则当作坏帧：只推进修订号，不改条目。
  if (!session || session.id !== id) return { feed: { ...feed, revision, updatedAt: now }, effects }
  effects.upserts.push(session)
  return {
    feed: {
      ...feed,
      revision,
      entries: {
        ...feed.entries,
        [id]: { session, card: parseBoardCard(frame.card), revision, receivedAt: now },
      },
      updatedAt: now,
    },
    effects,
  }
}
