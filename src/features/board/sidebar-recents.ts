// 侧栏“最近”（取代原来的“今天完成”）：上面状态组（需要你、运行中、排队）之外的未归档会话，
// 按工作区分组、组可收起。排序看“最后操作”的入栈序号（activityOrder：新建、发消息、打开会话），
// 不看时间——开发测试时会改系统时间；没入栈过的旧会话（序号 0）排在后面，再按修改时间。
// 组内置顶的在前；组之间按组里最近一次操作排；只留最近几个工作区、每组几条，更早的去“全部会话”。
// 纯函数，不读 store：侧栏、手机抽屉和测试共用。
import type { SessionSummary } from '@/types/chat'
import { sessionWorkspaceCwd, sessionWorkspaceKey } from '@/features/chat/session-workspaces'
import { boardSessionTitle, type BoardItem } from './board-model'
import { boardStatusGlyph } from './card-presentation'
import type { SidebarRecentRow } from './sidebar-model'

// “最近”最多列出的工作区个数，与每组默认显示的条数；当前打开的会话不受这两个限制。
export const SIDEBAR_RECENT_WORKSPACE_LIMIT = 6
export const SIDEBAR_RECENT_LIMIT = 5

type RecentSession = Pick<
  SessionSummary,
  'id' | 'name' | 'firstMessage' | 'modified' | 'archived' | 'unread' | 'pinned' | 'cwd'
> & {
  messageCount?: number
  botId?: string
  projectless?: boolean
  projectCwd?: string
  activityOrder?: number
}

export type SidebarRecentGroup = {
  // sessionWorkspaceKey：无项目会话统一是 NO_WORKSPACE_KEY。
  key: string
  cwd: string
  // 组内全部条数（标题上的计数）。
  count: number
  // 要画出来的行：收起时只剩正在看的会话（如果在这一组）。
  rows: SidebarRecentRow[]
  // 没收起、也没点“显示更多”时被截掉的条数。
  hidden: number
  collapsed: boolean
  expanded: boolean
}

function activityOrder(session: RecentSession) {
  const value = session.activityOrder
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? value : 0
}

// 最近操作在前：先比入栈序号，都没入栈过再比修改时间，最后按 id 保证稳定。
function byRecency(left: SidebarRecentRow, right: SidebarRecentRow) {
  return (
    right.order - left.order ||
    (right.at ?? -Infinity) - (left.at ?? -Infinity) ||
    (left.id < right.id ? -1 : left.id > right.id ? 1 : 0)
  )
}

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
    collapsed = new Set(),
    expanded = new Set(),
    limit = SIDEBAR_RECENT_LIMIT,
    workspaceLimit = SIDEBAR_RECENT_WORKSPACE_LIMIT,
  }: {
    // 看板卡片：有卡片的会话沿用它的状态与 Bot 头像。
    items: readonly Pick<BoardItem, 'id' | 'status' | 'lane' | 'bot'>[]
    // 上面状态组里已经列出的会话。
    shownIds: ReadonlySet<string>
    currentId: string
    // 收起的工作区、点了“显示更多”的工作区（sessionWorkspaceKey）。
    collapsed?: ReadonlySet<string>
    expanded?: ReadonlySet<string>
    limit?: number
    workspaceLimit?: number
  },
): { groups: SidebarRecentGroup[]; rows: SidebarRecentRow[]; hiddenWorkspaces: number } {
  const cards = new Map(items.map((item) => [item.id, item]))
  const seen = new Set<string>()
  const byWorkspace = new Map<string, { key: string; cwd: string; rows: SidebarRecentRow[] }>()
  for (const session of sessions) {
    if (!session || typeof session.id !== 'string' || !session.id || seen.has(session.id)) continue
    seen.add(session.id)
    if (session.archived === true || shownIds.has(session.id)) continue
    // Bot 的长期会话在侧栏的 Bots 分区里，不在“最近”里再列一遍。
    if (session.botId) continue
    // 没发过消息的空草稿不算会话（与看板、历史会话一致）；正在看的那个除外，侧栏里得找得到它。
    if (
      typeof session.messageCount === 'number' &&
      session.messageCount <= 0 &&
      session.id !== currentId
    )
      continue
    const card = cards.get(session.id)
    const key = sessionWorkspaceKey(session)
    const workspace = byWorkspace.get(key) ?? { key, cwd: sessionWorkspaceCwd(session), rows: [] }
    workspace.rows.push({
      group: 'recent',
      id: session.id,
      title: boardSessionTitle(session as SessionSummary),
      ...(card?.bot ? { bot: card.bot } : {}),
      status: card?.status ?? 'done',
      pinned: session.pinned === true,
      glyph: card ? boardStatusGlyph(card) : 'done',
      at: modifiedAt(session),
      order: activityOrder(session),
      unread: session.unread === true,
    })
    byWorkspace.set(key, workspace)
  }
  // 组的先后看组里最近操作的那条（置顶不算：置顶只在组内排前，不把整组顶上去）。
  const ranked = [...byWorkspace.values()]
    .map((workspace) => {
      const latest = [...workspace.rows].sort(byRecency)[0]
      workspace.rows.sort(
        (left, right) => Number(right.pinned) - Number(left.pinned) || byRecency(left, right),
      )
      return { ...workspace, latest }
    })
    .sort((left, right) => byRecency(left.latest, right.latest))
  // 只留最近的几个工作区；正在看的会话所在的组例外。
  const kept = ranked.filter(
    (workspace, index) =>
      index < workspaceLimit || workspace.rows.some((row) => row.id === currentId),
  )
  const groups = kept.map((workspace): SidebarRecentGroup => {
    const isCollapsed = collapsed.has(workspace.key)
    const isExpanded = !isCollapsed && expanded.has(workspace.key)
    const rows = isCollapsed
      ? workspace.rows.filter((row) => row.id === currentId)
      : isExpanded
        ? workspace.rows
        : workspace.rows.filter((row, index) => index < limit || row.id === currentId)
    return {
      key: workspace.key,
      cwd: workspace.cwd,
      count: workspace.rows.length,
      rows,
      hidden: isCollapsed || isExpanded ? 0 : workspace.rows.length - rows.length,
      collapsed: isCollapsed,
      expanded: isExpanded,
    }
  })
  return {
    groups,
    rows: groups.flatMap((group) => group.rows),
    hiddenWorkspaces: ranked.length - kept.length,
  }
}
