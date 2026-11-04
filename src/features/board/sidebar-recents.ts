// 侧栏“最近”（取代原来的“今天完成”）：上面状态组（需要你、运行中、排队）之外的未归档会话，
// 置顶的在前，其余按最近修改从新到旧，只留前几条；更早的去“全部会话”（历史会话页）找。
// 侧栏不按项目分组（项目多了侧栏就成了文件树）：按项目看用全部代理页的项目筛选。
// 纯函数，不读 store：侧栏、手机抽屉和测试共用。
import type { SessionSummary } from '@/types/chat'
import { boardSessionTitle, type BoardItem } from './board-model'
import { boardStatusGlyph } from './card-presentation'
import type { SidebarRecentRow } from './sidebar-model'

// “最近”显示的条数；当前打开的会话排在后面时另外补上。
export const SIDEBAR_RECENT_LIMIT = 8

type RecentSession = Pick<
  SessionSummary,
  'id' | 'name' | 'firstMessage' | 'modified' | 'archived' | 'unread' | 'pinned'
> & { messageCount?: number }

function modifiedAt(session: RecentSession) {
  const value = Date.parse(session.modified || '')
  return Number.isFinite(value) ? value : null
}

export function buildSidebarRecents(
  sessions: readonly RecentSession[],
  {
    items,
    shownIds,
    currentId,
    limit = SIDEBAR_RECENT_LIMIT,
  }: {
    // 看板卡片：有卡片的会话沿用它的状态与 Bot 头像。
    items: readonly Pick<BoardItem, 'id' | 'status' | 'lane' | 'bot'>[]
    // 上面状态组里已经列出的会话。
    shownIds: ReadonlySet<string>
    currentId: string
    limit?: number
  },
): { rows: SidebarRecentRow[]; hidden: number } {
  const cards = new Map(items.map((item) => [item.id, item]))
  const seen = new Set<string>()
  const rows: SidebarRecentRow[] = []
  for (const session of sessions) {
    if (!session || typeof session.id !== 'string' || !session.id || seen.has(session.id)) continue
    seen.add(session.id)
    if (session.archived === true || shownIds.has(session.id)) continue
    // 没发过消息的空草稿不算会话（与看板、历史会话一致）；正在看的那个除外，侧栏里得找得到它。
    if (
      typeof session.messageCount === 'number' &&
      session.messageCount <= 0 &&
      session.id !== currentId
    )
      continue
    const card = cards.get(session.id)
    rows.push({
      group: 'recent',
      id: session.id,
      title: boardSessionTitle(session as SessionSummary),
      ...(card?.bot ? { bot: card.bot } : {}),
      status: card?.status ?? 'done',
      pinned: session.pinned === true,
      glyph: card ? boardStatusGlyph(card) : 'done',
      at: modifiedAt(session),
      unread: session.unread === true,
    })
  }
  rows.sort(
    (left, right) =>
      Number(right.pinned) - Number(left.pinned) ||
      (right.at ?? -Infinity) - (left.at ?? -Infinity) ||
      (left.id < right.id ? -1 : left.id > right.id ? 1 : 0),
  )
  // 正在看的会话排在截断之后也留着：不能让它在侧栏里找不到。
  const shown = rows.filter((row, index) => index < limit || row.id === currentId)
  return { rows: shown, hidden: rows.length - shown.length }
}
