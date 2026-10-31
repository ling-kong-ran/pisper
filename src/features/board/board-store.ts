// 看板数据层。会话目录的唯一权威所有者是 react-query 的 ['sessions','board']（见 startup-queries）；
// 本模块的 zustand 覆盖层只保存服务端列表没有的实时细节：看板条目与 card（事件流 / 看板视图）、
// 审批详情（/live）、本客户端发起的运行活动、乐观补丁和进行中的动作。
// 数据源按 Runtime 能力三选一（规格 8.6 降级矩阵）：
// - board.events：每个标签页只开一条 /api/board/events，由本 store 独占；页面隐藏时断开，可见后带
//   ?after=revision 补齐；resync_required 时等新快照整体替换并补读目录；HTTP 拒绝（如 429）时
//   暂停 60 秒、退回 view=board 轮询。审批详情来自 card.approvals，只有预览被截断时才取 /live。
// - board.view（没有 events）：页面可见时每 2 秒读一次 view=board。
// - 都没有（旧 Runtime）：第 1 期路径，页面可见时每 3 秒刷新 /api/sessions，需要你的审批卡单独取 /live。
// 轮询、事件流、/live 请求与流消费只在有视图持有（retainBoardSync）时运行，最后一个持有者释放后统一清理。
import { hashKey } from '@tanstack/react-query'
import { create } from 'zustand'
import { deriveBoardStatus } from '@shared/session-board-status.mjs'
import { ApiError } from '@/lib/http'
import {
  fetchStartupQuery,
  invalidateStartupQuery,
  queryClient,
  SESSIONS_QUERY_KEY,
  startupQueryOptions,
} from '@/lib/startup-queries'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import type { SessionSummary } from '@/types/chat'
import {
  boardApi,
  parseBoardApproval,
  summarizeToolArgs,
  type BoardActivity,
  type BoardApproval,
  type BoardStreamEvent,
} from './board-api'
import {
  applyBoardFeedEvent,
  emptyBoardFeed,
  replaceBoardFeed,
  type BoardFeedEffects,
  type BoardFeedEntry,
  type BoardFeedMode,
  type BoardFeedState,
} from './board-feed'
import { notifyComponentFeedEvent } from './component-changes'

export const BOARD_SESSIONS_QUERY_KEY = SESSIONS_QUERY_KEY
export const BOARD_POLL_INTERVAL_MS = 3_000
// 有 board.view 没有 board.events 时，看板视图的轮询间隔（规格 8.6）。
export const BOARD_VIEW_POLL_INTERVAL_MS = 2_000
export const BOARD_VIEW_MAX_BACKOFF_MS = 30_000
// 事件流断开后的重连退避：正常结束（收到过数据）按基础间隔重连，连续失败按指数退避。
export const BOARD_EVENTS_RETRY_BASE_MS = 1_000
export const BOARD_EVENTS_RETRY_MAX_MS = 30_000
// 事件流被 HTTP 拒绝（连接数已满、Runtime 不支持）后暂停这么久，期间退回 view=board 轮询。
export const BOARD_EVENTS_BLOCK_MS = 60_000
// 内联动作完成后等待事件流带回确认帧的上限；超时就以当时的数据为准，不再等。
export const BOARD_CONFIRM_TIMEOUT_MS = 4_000
// 事件流空闲看门狗：Runtime 每 15 秒发一次心跳，约 2.5 个心跳周期没有任何帧就当作半开连接
// （切换网络、休眠唤醒、代理静默丢连接），断开后带 after 重连。
export const BOARD_EVENTS_IDLE_TIMEOUT_MS = 40_000
// 条目离开看板（归档、删除）或需要重新同步时，合并 300ms 再补读一次完整目录。
const CATALOG_REFRESH_DELAY_MS = 300
// 连接预算：Runtime 走 HTTP/1.1，Chromium、WebView2、WKWebView 对同一主机最多 6 条连接。
// 看板长期占用的连接（事件流 1 条 + 本客户端持有的运行流 + 在途的 /live）合计不超过 4 条，
// 给轮询、审批提交与其他页面请求至少留 2 条，否则连续派发几个长任务后整个应用会排队卡死。
export const BOARD_CONNECTION_BUDGET = 4
// 最多持有 2 条运行流（给出实时活动行）；更早的流在 Runtime 受理后释放，run 在 Runtime 继续，
// 卡片退回降级形态，由列表轮询或事件流反映真实状态。
export const BOARD_HELD_STREAM_LIMIT = 2
// 降级矩阵：/live 并发上限，错开发起，避免一面需要你的墙同时压垮 Runtime；还受上面的连接预算约束。
export const BOARD_LIVE_LIMIT = 3
export const BOARD_LIVE_STAGGER_MS = 250
export const BOARD_LIVE_REFRESH_MS = 6_000
export const BOARD_LIVE_MAX_BACKOFF_MS = 60_000
// 路由切换时旧视图先卸载、新视图后挂载；短暂宽限避免把轮询和流消费停掉又重开。
export const BOARD_RELEASE_GRACE_MS = 1_000

export type BoardSessionsData = { sessions: SessionSummary[] }

export type BoardLiveEntry = {
  status: 'loading' | 'ready' | 'error'
  approvals: BoardApproval[]
  error: string
  activity: BoardActivity | null
  startedAt: string | null
  fetchedAt: number | null
  failures: number
}

export type BoardRunKind = 'retry' | 'launch'

export type BoardLocalRun = {
  token: number
  kind: BoardRunKind
  runId: string
  startedAt: number
  activity: BoardActivity | null
}

export type BoardSessionPatch = Partial<
  Pick<
    SessionSummary,
    'pinned' | 'archived' | 'unread' | 'needsAttention' | 'attentionReason' | 'streaming'
  >
>

export type BoardActionKind =
  'approve' | 'deny' | 'retry' | 'markHandled' | 'pin' | 'archive' | 'rename' | 'answer' | 'start'

export type BoardOverlayState = {
  // 第 2 期 Runtime 的看板条目与 card；旧 Runtime（legacy）时为空且 ready 为 false。
  feed: BoardFeedState
  live: Record<string, BoardLiveEntry>
  localRuns: Record<string, BoardLocalRun>
  // 首次观察到运行的时间：降级模式拿不到真实开始时间，用它保持运行中泳道顺序稳定。
  runningSince: Record<string, number>
  patches: Record<string, Array<{ token: number; patch: BoardSessionPatch }>>
  pending: Record<string, BoardActionKind>
}

const EMPTY_OVERLAY: BoardOverlayState = {
  feed: emptyBoardFeed(),
  live: {},
  localRuns: {},
  runningSince: {},
  patches: {},
  pending: {},
}

export const useBoardOverlay = create<BoardOverlayState>()(() => ({ ...EMPTY_OVERLAY }))

export function boardSessionsQueryOptions() {
  return startupQueryOptions<BoardSessionsData>('sessions')
}

export function readBoardSessions(): SessionSummary[] {
  return queryClient.getQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY)?.sessions ?? []
}

// 显式刷新：与其他读者共用同一个在途请求和同一份缓存。
export function refreshBoardSessions() {
  return fetchStartupQuery<BoardSessionsData>('sessions', true)
}

// 写操作完成后的确认刷新：先推进版本让动作之前发出的旧请求作废并补读，再等新快照落地。
export async function confirmBoardSessions() {
  void invalidateStartupQuery('sessions')
  await refreshBoardSessions()
}

export type BoardConfirmPredicate = (entry: BoardFeedEntry | undefined) => boolean

/**
 * 内联动作之后确认这一张卡片的服务端状态，按数据源区分：
 * - 事件流在线：等这张卡片的下一帧满足 predicate（例如审批不再出现、开始运行），最多等 4 秒；
 * - 看板视图（或事件流暂时断开）：立即读一次 view=board；
 * - 旧 Runtime：刷新完整目录（第 1 期行为）。
 * 从不抛错；调用方在它结束后撤掉乐观补丁。
 */
export async function confirmBoardSession(
  sessionId: string,
  predicate: BoardConfirmPredicate = () => true,
  // fresh：只认动作之后新到的帧。审批用它：动作前的条目可能是请求途中算出的旧状态。
  { fresh = false }: { fresh?: boolean } = {},
) {
  if (sync.active && sync.mode === 'events' && sync.events.controller) {
    if (!fresh && predicate(useBoardOverlay.getState().feed.entries[sessionId])) return
    await waitForFeedEntry(sessionId, predicate, BOARD_CONFIRM_TIMEOUT_MS)
    return
  }
  if (sync.active && sync.mode !== 'legacy' && boardFeatures().view) {
    await pollViewFresh()
    return
  }
  await confirmBoardSessions().catch(() => undefined)
}

function waitForFeedEntry(sessionId: string, predicate: BoardConfirmPredicate, timeoutMs: number) {
  return new Promise<void>((resolve) => {
    const initial = useBoardOverlay.getState().feed.entries[sessionId]
    let timer: ReturnType<typeof setTimeout> | null = null
    let unsubscribe = () => {}
    const finish = () => {
      if (timer) clearTimeout(timer)
      unsubscribe()
      resolve()
    }
    // 只认新到的帧：等待开始时已有的条目不再重复判断。
    unsubscribe = useBoardOverlay.subscribe((state) => {
      const entry = state.feed.entries[sessionId]
      if (entry !== initial && predicate(entry)) finish()
    })
    timer = setTimeout(finish, timeoutMs)
  })
}

// 看板数据源需要的 Runtime 能力：旧 Runtime 没有这两个键，按 false 处理。
function boardFeatures() {
  const { capabilities, loaded } = useRuntimeCapabilitiesStore.getState()
  const features: Partial<Record<string, boolean>> = capabilities.features
  return {
    loaded,
    view: loaded && features['board.view'] === true,
    events: loaded && features['board.events'] === true,
  }
}

// 标记已处理：Runtime 支持 board.view（第 2 期）时同时清掉失败注意力，否则只标已读。
export function boardDismissAttentionAvailable() {
  return boardFeatures().view
}

// —— 乐观补丁：只叠加在看板派生视图上，权威缓存始终是服务端快照 ——
let patchToken = 0

export function applyBoardPatch(sessionId: string, patch: BoardSessionPatch) {
  const token = ++patchToken
  useBoardOverlay.setState((state) => ({
    patches: {
      ...state.patches,
      [sessionId]: [...(state.patches[sessionId] ?? []), { token, patch }],
    },
  }))
  return token
}

export function clearBoardPatch(sessionId: string, token: number) {
  useBoardOverlay.setState((state) => {
    const current = state.patches[sessionId]
    if (!current?.some((entry) => entry.token === token)) return state
    const remaining = current.filter((entry) => entry.token !== token)
    const patches = { ...state.patches }
    if (remaining.length) patches[sessionId] = remaining
    else delete patches[sessionId]
    return { patches }
  })
}

export function patchedSession(
  session: SessionSummary,
  patches: BoardOverlayState['patches'],
): SessionSummary {
  const entries = patches[session.id]
  if (!entries?.length) return session
  return entries.reduce<SessionSummary>(
    (current, entry) => ({ ...current, ...entry.patch }),
    session,
  )
}

// —— 进行中的动作：同一按钮重复触发直接拒绝，界面据此禁用 ——
// 允许与拒绝同一条审批互斥；其他动作按会话与动作类型区分。
export function boardActionKey(kind: BoardActionKind, sessionId: string, approvalId = '') {
  return kind === 'approve' || kind === 'deny'
    ? `approval:${sessionId}:${approvalId}`
    : `${kind}:${sessionId}`
}

export function beginBoardAction(key: string, kind: BoardActionKind) {
  if (useBoardOverlay.getState().pending[key]) return false
  useBoardOverlay.setState((state) => ({ pending: { ...state.pending, [key]: kind } }))
  return true
}

export function endBoardAction(key: string) {
  useBoardOverlay.setState((state) => {
    if (!(key in state.pending)) return state
    const pending = { ...state.pending }
    delete pending[key]
    return { pending }
  })
}

function updateLive(
  sessionId: string,
  update: (entry: BoardLiveEntry | undefined) => BoardLiveEntry | null,
) {
  useBoardOverlay.setState((state) => {
    const next = update(state.live[sessionId])
    if (next === state.live[sessionId]) return state
    const live = { ...state.live }
    if (next) live[sessionId] = next
    else delete live[sessionId]
    return { live }
  })
}

// 看板条目（card）里的审批：乐观移除与失败回滚都要同时作用在它和 /live 详情上。
function updateFeedApprovals(
  sessionId: string,
  update: (approvals: BoardApproval[]) => BoardApproval[],
) {
  useBoardOverlay.setState((state) => {
    const entry = state.feed.entries[sessionId]
    if (!entry) return state
    const approvals = update(entry.card.approvals)
    if (approvals === entry.card.approvals) return state
    return {
      feed: {
        ...state.feed,
        entries: {
          ...state.feed.entries,
          [sessionId]: { ...entry, card: { ...entry.card, approvals } },
        },
      },
    }
  })
}

export function removeBoardApproval(sessionId: string, approvalId: string) {
  updateLive(sessionId, (entry) =>
    entry && entry.approvals.some((approval) => approval.id === approvalId)
      ? { ...entry, approvals: entry.approvals.filter((approval) => approval.id !== approvalId) }
      : (entry ?? null),
  )
  updateFeedApprovals(sessionId, (approvals) =>
    approvals.some((approval) => approval.id === approvalId)
      ? approvals.filter((approval) => approval.id !== approvalId)
      : approvals,
  )
}

export function restoreBoardApproval(sessionId: string, approval: BoardApproval) {
  // 有看板条目时审批成员以 card 为准，回滚写回 card；否则写回 /live 详情（第 1 期路径）。
  if (useBoardOverlay.getState().feed.entries[sessionId]) {
    updateFeedApprovals(sessionId, (approvals) =>
      approvals.some((item) => item.id === approval.id) ? approvals : [...approvals, approval],
    )
    return
  }
  updateLive(sessionId, (entry) => {
    const base = entry ?? emptyLiveEntry('ready')
    if (base.approvals.some((item) => item.id === approval.id)) return base
    return { ...base, approvals: [...base.approvals, approval] }
  })
}

// 一条审批的可批准详情：/live 取到的完整详情优先；card 里的审批只有预览完整（没被截断）时才算数。
export function boardApprovalDetail(sessionId: string, approvalId: string) {
  const state = useBoardOverlay.getState()
  const live = state.live[sessionId]?.approvals.find((approval) => approval.id === approvalId)
  if (live) return live
  const card = state.feed.entries[sessionId]?.card.approvals.find(
    (approval) => approval.id === approvalId,
  )
  return card && !card.truncated ? card : undefined
}

// 服务端确认过的会话字段（整理接口的响应）同步写进看板条目，不必等下一帧才反映到卡片上。
export function updateBoardFeedSession(sessionId: string, fields: Partial<SessionSummary>) {
  useBoardOverlay.setState((state) => {
    const entry = state.feed.entries[sessionId]
    if (!entry) return state
    return {
      feed: {
        ...state.feed,
        entries: {
          ...state.feed.entries,
          // 服务端刚确认的字段：这一行此刻是最新的，目录重读时以它为准。
          [sessionId]: {
            ...entry,
            session: { ...entry.session, ...fields },
            receivedAt: Date.now(),
          },
        },
      },
    }
  })
}

// 这张卡片当前还在等待的审批（用于判断处理完这一条之后运行是否恢复）。
export function boardSessionApprovals(sessionId: string): BoardApproval[] {
  const state = useBoardOverlay.getState()
  const entry = state.feed.entries[sessionId]
  if (entry && sync.mode !== 'legacy') return entry.card.approvals
  return state.live[sessionId]?.approvals ?? []
}

function emptyLiveEntry(status: BoardLiveEntry['status']): BoardLiveEntry {
  return {
    status,
    approvals: [],
    error: '',
    activity: null,
    startedAt: null,
    fetchedAt: null,
    failures: 0,
  }
}

// —— 同步生命周期：单一所有者 ——
type EventsRuntime = {
  // 当前这条 /api/board/events 连接；null 表示没有连接（隐藏、退避中或不是事件流模式）。
  controller: AbortController | null
  retryTimer: ReturnType<typeof setTimeout> | null
  // 连续失败次数（没收到任何数据就断开），决定指数退避。
  attempt: number
  // HTTP 拒绝后暂停事件流到这个时间点，期间退回 view=board。
  blockedUntil: number
  blockTimer: ReturnType<typeof setTimeout> | null
  // 当前连接的空闲看门狗：每收到一帧（包括心跳）重新计时；连接结束或页面隐藏时清掉。
  watchdog: ReturnType<typeof setTimeout> | null
}

type SyncRuntime = {
  retainers: number
  active: boolean
  mode: BoardFeedMode
  pollTimer: ReturnType<typeof setTimeout> | null
  releaseTimer: ReturnType<typeof setTimeout> | null
  catalogTimer: ReturnType<typeof setTimeout> | null
  liveTimers: Map<string, ReturnType<typeof setTimeout>>
  liveControllers: Map<string, AbortController>
  liveNextAt: Map<string, number>
  // 按开始顺序排列（Map 保留插入顺序），超出上限时先释放最早的已受理流。
  runControllers: Map<string, HeldRun>
  events: EventsRuntime
  view: { inFlight: Promise<void> | null; controller: AbortController | null; failures: number }
  // 当前数据源的第一次请求已经有结果（成功或失败）：在这之前不为审批卡发 /live，免得与第一份 card 重复。
  feedSettled: boolean
  // 最近一次目录请求开始的本地时间：之后才收到的看板条目比这份目录新，更早的则更旧。
  catalogFetchAt: number
  cleanups: Array<() => void>
}

type HeldRun = { controller: AbortController; accepted: boolean }

const sync: SyncRuntime = {
  retainers: 0,
  active: false,
  mode: 'legacy',
  pollTimer: null,
  releaseTimer: null,
  catalogTimer: null,
  liveTimers: new Map(),
  liveControllers: new Map(),
  liveNextAt: new Map(),
  runControllers: new Map(),
  events: {
    controller: null,
    retryTimer: null,
    attempt: 0,
    blockedUntil: 0,
    blockTimer: null,
    watchdog: null,
  },
  view: { inFlight: null, controller: null, failures: 0 },
  feedSettled: false,
  catalogFetchAt: 0,
  cleanups: [],
}

function documentVisible() {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

// 按 Runtime 能力选择数据源；事件流被 HTTP 拒绝后在暂停期内退回看板视图。
function desiredMode(): BoardFeedMode {
  const features = boardFeatures()
  if (features.events && Date.now() >= sync.events.blockedUntil) return 'events'
  if (features.view) return 'view'
  return 'legacy'
}

// 看板条目可用：派生卡片以条目为准，审批详情来自 card。
function cardsActive(feed = useBoardOverlay.getState().feed) {
  return sync.mode !== 'legacy' && feed.ready
}

function clearPollTimer() {
  if (sync.pollTimer) clearTimeout(sync.pollTimer)
  sync.pollTimer = null
}

function schedulePoll(delay: number) {
  clearPollTimer()
  if (!sync.active || sync.mode === 'events' || !documentVisible()) return
  sync.pollTimer = setTimeout(() => {
    sync.pollTimer = null
    void pollOnce()
  }, delay)
}

function viewPollDelay() {
  return sync.view.failures
    ? Math.min(BOARD_VIEW_POLL_INTERVAL_MS * 2 ** sync.view.failures, BOARD_VIEW_MAX_BACKOFF_MS)
    : BOARD_VIEW_POLL_INTERVAL_MS
}

async function pollOnce() {
  const mode = sync.mode
  if (mode === 'view') await pollView()
  else {
    try {
      await refreshBoardSessions()
    } catch {
      // 失败保留在查询状态里供界面展示；上一份有效快照不被清空，下一轮继续尝试。
    }
  }
  if (!sync.active || sync.mode !== mode) return
  reconcileLiveDetails()
  schedulePoll(mode === 'view' ? viewPollDelay() : BOARD_POLL_INTERVAL_MS)
}

// 读一次 view=board 并整体替换看板条目；同一时刻只有一个在途请求，确认刷新与定时轮询共用它。
function pollView(): Promise<void> {
  if (sync.view.inFlight) return sync.view.inFlight
  const controller = new AbortController()
  sync.view.controller = controller
  const request = (async () => {
    try {
      const page = await boardApi.listBoardView(controller.signal)
      if (controller.signal.aborted || !sync.active) return
      sync.view.failures = 0
      sync.feedSettled = true
      // 事件流模式下也可能借用一次视图（事件流暂时断开时的确认刷新），修订号不覆盖事件流的。
      commitFeed(
        replaceBoardFeed(
          useBoardOverlay.getState().feed,
          page.items,
          sync.mode === 'events' ? null : page.revision,
        ),
      )
    } catch {
      if (controller.signal.aborted) return
      sync.view.failures += 1
      sync.feedSettled = true
      reconcileLiveDetails()
    } finally {
      if (sync.view.controller === controller) {
        sync.view.controller = null
        sync.view.inFlight = null
      }
    }
  })()
  sync.view.inFlight = request
  return request
}

// 动作之后的确认读取：在途的那一次可能在动作之前就发出了，等它结束再读一次新的。
async function pollViewFresh() {
  const previous = sync.view.inFlight
  if (previous) await previous
  await pollView()
}

function stopView() {
  sync.view.controller?.abort()
  sync.view.controller = null
  sync.view.inFlight = null
}

// —— 看板事件流：每个标签页一条，本 store 独占 ——
function stopEvents() {
  const events = sync.events
  if (events.retryTimer) clearTimeout(events.retryTimer)
  events.retryTimer = null
  clearEventsWatchdog()
  const controller = events.controller
  events.controller = null
  controller?.abort()
}

function clearEventsWatchdog() {
  if (sync.events.watchdog) clearTimeout(sync.events.watchdog)
  sync.events.watchdog = null
}

function armEventsWatchdog(controller: AbortController) {
  clearEventsWatchdog()
  sync.events.watchdog = setTimeout(() => {
    sync.events.watchdog = null
    const events = sync.events
    if (events.controller !== controller || controller.signal.aborted) return
    // 半开连接：fetch 还挂着却不再有任何帧。先让出连接再中止，读循环的收尾因此不接手，
    // 由这里按正常结束处理：立即带 after 重连。
    events.controller = null
    controller.abort()
    if (!sync.active || sync.mode !== 'events') return
    scheduleEventsReconnect(0)
    reconcileLiveDetails()
  }, BOARD_EVENTS_IDLE_TIMEOUT_MS)
}

function scheduleEventsReconnect(delay: number) {
  const events = sync.events
  if (events.retryTimer) clearTimeout(events.retryTimer)
  events.retryTimer = setTimeout(() => {
    events.retryTimer = null
    connectEvents()
  }, delay)
}

// 4xx 是 Runtime 明确拒绝（连接数已满 429、不支持 404/409）：换个数据源，而不是原地重试。
function rejectedByRuntime(error: unknown) {
  return (
    error instanceof ApiError &&
    error.kind === 'http' &&
    typeof error.status === 'number' &&
    error.status >= 400 &&
    error.status < 500
  )
}

function blockEvents() {
  const events = sync.events
  events.blockedUntil = Date.now() + BOARD_EVENTS_BLOCK_MS
  if (events.blockTimer) clearTimeout(events.blockTimer)
  events.blockTimer = setTimeout(() => {
    events.blockTimer = null
    applyMode()
  }, BOARD_EVENTS_BLOCK_MS + 10)
  applyMode()
}

function connectEvents() {
  const events = sync.events
  if (!sync.active || sync.mode !== 'events' || events.controller || events.retryTimer) return
  if (!documentVisible()) return
  const controller = new AbortController()
  events.controller = controller
  armEventsWatchdog(controller)
  const feed = useBoardOverlay.getState().feed
  // 已有可信状态时带 after 补发；等待快照（resync / flush_failed）或还没有状态时要完整快照。
  const after = feed.ready && !feed.awaitingSnapshot ? feed.revision : null
  let progressed = false
  let reconnectNow = false
  void boardApi
    .openBoardEvents(
      after,
      (event, data) => {
        if (controller.signal.aborted || events.controller !== controller) return false
        armEventsWatchdog(controller)
        progressed = true
        events.attempt = 0
        sync.feedSettled = true
        const effects = handleFeedEvent(event, data)
        if (effects.reconnect) {
          reconnectNow = true
          return false
        }
        return undefined
      },
      controller.signal,
    )
    .then(
      () => null,
      (error: unknown) => error,
    )
    .then((error) => {
      if (events.controller !== controller) return
      events.controller = null
      clearEventsWatchdog()
      // 主动中止（页面隐藏、模式切换、停止同步）由中止方负责后续，这里不重连。
      if (controller.signal.aborted || !sync.active) return
      sync.feedSettled = true
      if (error && rejectedByRuntime(error)) {
        blockEvents()
      } else {
        const delay = reconnectNow
          ? 0
          : progressed
            ? BOARD_EVENTS_RETRY_BASE_MS
            : Math.min(BOARD_EVENTS_RETRY_BASE_MS * 2 ** events.attempt, BOARD_EVENTS_RETRY_MAX_MS)
        if (!progressed && !reconnectNow) events.attempt += 1
        scheduleEventsReconnect(delay)
      }
      // 空出的连接让给排队的 /live。
      reconcileLiveDetails()
    })
}

function handleFeedEvent(event: string, data: unknown): BoardFeedEffects {
  const result = applyBoardFeedEvent(useBoardOverlay.getState().feed, event, data)
  // 心跳只推进修订号：读者只订阅条目与分钟级时钟，不会因此重算；也不必重新安排 /live。
  commitFeed(
    result,
    event === 'heartbeat' || event === 'component_changed' || event === 'catalog_changed',
  )
  // 组件宿主按 component_changed 作废组件查询；重连快照可能跨过了组件事件，再通知一次 '*'。
  notifyComponentFeedEvent(event, data, result.effects.changed)
  return result.effects
}

function commitFeed(
  { feed, effects }: { feed: BoardFeedState; effects: BoardFeedEffects },
  quiet = false,
) {
  if (effects.changed) useBoardOverlay.setState({ feed: { ...feed, mode: sync.mode } })
  if (effects.upserts.length) upsertCatalogRows(effects.upserts)
  // catalog_changed（A9）：看板外的会话改名、归档、删除没有卡片增量，靠它补读目录，事件流在线时不再轮询目录。
  if (effects.removed.length || effects.resync || effects.catalog) scheduleCatalogRefresh()
  if (effects.changed && !quiet) reconcileLiveDetails()
}

// 事件流 / 看板视图带来的会话行写回权威目录：只改有变化的行，新出现的会话放在最前；
// 目录还没加载时不凭看板条目造一份残缺目录（历史、侧栏需要完整列表），等首次读取。
function upsertCatalogRows(rows: readonly SessionSummary[]) {
  const current = queryClient.getQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY)
  if (!current || !rows.length) return
  const index = new Map(current.sessions.map((session, position) => [session.id, position]))
  let sessions: SessionSummary[] | null = null
  const added: SessionSummary[] = []
  for (const row of rows) {
    const position = index.get(row.id)
    if (position === undefined) {
      if (!added.some((item) => item.id === row.id)) added.push(row)
      continue
    }
    const existing = (sessions ?? current.sessions)[position]
    if (JSON.stringify(existing) === JSON.stringify(row)) continue
    sessions ??= [...current.sessions]
    sessions[position] = row
  }
  // 没有变化时不写：写缓存会再触发一次 success 订阅，必须保证这里收敛。
  if (!sessions && !added.length) return
  queryClient.setQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY, {
    ...current,
    sessions: [...added, ...(sessions ?? current.sessions)],
  })
}

function scheduleCatalogRefresh() {
  if (sync.catalogTimer || !sync.active) return
  sync.catalogTimer = setTimeout(() => {
    sync.catalogTimer = null
    if (sync.active) void refreshBoardSessions().catch(() => undefined)
  }, CATALOG_REFRESH_DELAY_MS)
}

// 能力变化（启动时加载完成、远程 Runtime 切换）或事件流暂停到期时重新选择数据源。
function applyMode() {
  if (!sync.active) return
  const next = desiredMode()
  if (next === sync.mode) return
  sync.mode = next
  sync.feedSettled = false
  sync.view.failures = 0
  stopEvents()
  stopView()
  clearPollTimer()
  useBoardOverlay.setState((state) => ({
    // 退回旧 Runtime 路径时丢掉条目：它们不会再更新。换到另一种第 2 期数据源时保留到新数据替换。
    feed: next === 'legacy' ? emptyBoardFeed('legacy') : { ...state.feed, mode: next },
  }))
  if (next === 'events') {
    sync.events.attempt = 0
    connectEvents()
  } else schedulePoll(0)
  reconcileLiveDetails()
}

// 持有看板同步：任何读取看板数据的视图挂载时调用，返回幂等的释放函数。
export function retainBoardSync() {
  sync.retainers += 1
  if (sync.releaseTimer) {
    clearTimeout(sync.releaseTimer)
    sync.releaseTimer = null
  }
  if (!sync.active) startSync()
  let released = false
  return () => {
    if (released) return
    released = true
    sync.retainers = Math.max(0, sync.retainers - 1)
    if (sync.retainers > 0) return
    sync.releaseTimer = setTimeout(() => {
      sync.releaseTimer = null
      if (sync.retainers === 0) stopBoardSync()
    }, BOARD_RELEASE_GRACE_MS)
  }
}

function startSync() {
  sync.active = true
  sync.mode = desiredMode()
  sync.feedSettled = false
  sync.view.failures = 0
  sync.events.attempt = 0
  useBoardOverlay.setState((state) => ({
    feed: sync.mode === 'legacy' ? emptyBoardFeed('legacy') : { ...state.feed, mode: sync.mode },
  }))
  const sessionsHash = hashKey(BOARD_SESSIONS_QUERY_KEY)
  // 停止期间别的读者可能重读过目录（没有被下面的订阅记到）：以缓存的更新时间兜底，
  // 留下来的旧条目不会把更新的目录改回去。
  sync.catalogFetchAt = Math.max(
    sync.catalogFetchAt,
    queryClient.getQueryState(BOARD_SESSIONS_QUERY_KEY)?.dataUpdatedAt ?? 0,
  )
  sync.cleanups.push(
    queryClient.getQueryCache().subscribe((event) => {
      if (event.type !== 'updated' || event.query.queryHash !== sessionsHash) return
      if (event.action.type === 'fetch') {
        sync.catalogFetchAt = Date.now()
        return
      }
      if (event.action.type !== 'success') return
      reconcileSessions()
    }),
  )
  sync.cleanups.push(useRuntimeCapabilitiesStore.subscribe(() => applyMode()))
  if (typeof document !== 'undefined') {
    const visibilityChanged = () => {
      if (documentVisible()) {
        if (sync.mode === 'events') connectEvents()
        else schedulePoll(0)
        reconcileLiveDetails()
        return
      }
      // 页面隐藏：断开事件流、停掉轮询；回到前台时带 after 补齐。
      clearPollTimer()
      stopEvents()
    }
    document.addEventListener('visibilitychange', visibilityChanged)
    sync.cleanups.push(() => document.removeEventListener('visibilitychange', visibilityChanged))
  }
  reconcileSessions()
  if (sync.mode === 'events') connectEvents()
  else if (sync.mode === 'view') schedulePoll(0)
  else {
    // 有新鲜快照时不必立刻重复请求；陈旧或缺失时马上补读。
    const updatedAt = queryClient.getQueryState(BOARD_SESSIONS_QUERY_KEY)?.dataUpdatedAt ?? 0
    schedulePoll(Math.max(0, updatedAt + BOARD_POLL_INTERVAL_MS - Date.now()))
  }
}

// 停止全部同步工作：轮询、事件流、/live 请求与本客户端持有的流消费。Runtime 的 run 不受影响。
// 看板条目与修订号保留：下一次持有时事件流带 after 补发，而不是从空白开始。
export function stopBoardSync() {
  sync.active = false
  clearPollTimer()
  if (sync.releaseTimer) clearTimeout(sync.releaseTimer)
  if (sync.catalogTimer) clearTimeout(sync.catalogTimer)
  if (sync.events.blockTimer) clearTimeout(sync.events.blockTimer)
  sync.releaseTimer = null
  sync.catalogTimer = null
  sync.events.blockTimer = null
  stopEvents()
  stopView()
  for (const cleanup of sync.cleanups.splice(0)) cleanup()
  for (const timer of sync.liveTimers.values()) clearTimeout(timer)
  for (const controller of sync.liveControllers.values()) controller.abort()
  for (const run of sync.runControllers.values()) run.controller.abort()
  sync.liveTimers.clear()
  sync.liveControllers.clear()
  sync.liveNextAt.clear()
  sync.runControllers.clear()
  useBoardOverlay.setState({ live: {}, localRuns: {}, runningSince: {} })
}

export function boardSyncActive() {
  return sync.active
}

// 供测试与诊断：当前数据源、事件流是否在线。
export function boardFeedMode() {
  return sync.mode
}

export function boardEventsConnected() {
  return sync.events.controller !== null
}

// 显式刷新看板数据：目录总是补读；第 2 期数据源再读一次看板视图（事件流在线时由增量负责）。
export async function refreshBoardFeed() {
  const catalog = refreshBoardSessions()
  if (sync.active && sync.mode === 'view') await pollViewFresh()
  await catalog
}

// 列表变化后的覆盖层整理：记录运行起点、清掉已消失会话的细节，并为新出现的审批卡安排 /live。
function reconcileSessions() {
  const sessions = readBoardSessions()
  const now = Date.now()
  const { runningSince, localRuns, feed } = useBoardOverlay.getState()
  const sources = cardsActive(feed)
    ? [...Object.values(feed.entries).map((entry) => entry.session), ...sessions]
    : sessions
  const next: Record<string, number> = {}
  let changed = false
  for (const session of sources) {
    if (session.streaming !== true || session.id in next) continue
    next[session.id] = runningSince[session.id] ?? localRuns[session.id]?.startedAt ?? now
    if (!(session.id in runningSince)) changed = true
  }
  if (!changed) changed = Object.keys(runningSince).some((id) => !(id in next))
  if (changed) useBoardOverlay.setState({ runningSince: next })
  if (cardsActive(feed)) syncEntriesWithCatalog(sessions)
  reconcileLiveDetails()
}

// 目录与看板条目以较新者为准：在这次目录请求开始之后才收到的条目行写回目录（目录可能比
// 事件流旧）；更早收到的条目行反过来由目录刷新（例如改名后的目录重读不会被旧条目改回去，
// 运行结束后的目录也不会被条目里残留的“运行中”覆盖）。card 只在条目里，不受影响。
function syncEntriesWithCatalog(sessions: readonly SessionSummary[]) {
  const fetchedAt = sync.catalogFetchAt
  const catalog = new Map(sessions.map((session) => [session.id, session]))
  const newer: SessionSummary[] = []
  const refreshed: Record<string, SessionSummary> = {}
  for (const entry of Object.values(useBoardOverlay.getState().feed.entries)) {
    if ((entry.receivedAt ?? Number.POSITIVE_INFINITY) >= fetchedAt) {
      newer.push(entry.session)
      continue
    }
    const row = catalog.get(entry.session.id)
    if (row && row !== entry.session && JSON.stringify(row) !== JSON.stringify(entry.session))
      refreshed[row.id] = row
  }
  if (Object.keys(refreshed).length)
    useBoardOverlay.setState((state) => {
      const entries = { ...state.feed.entries }
      for (const [id, row] of Object.entries(refreshed))
        if (entries[id]) entries[id] = { ...entries[id], session: row }
      return { feed: { ...state.feed, entries } }
    })
  upsertCatalogRows(newer)
}

function approvalSessionIds() {
  const { patches, feed } = useBoardOverlay.getState()
  const ids = new Set<string>()
  const now = Date.now()
  if (cardsActive(feed)) {
    // 第 2 期：审批详情来自 card；只有预览被截断（或 card 还没带上审批）时才需要 /live 的完整参数。
    for (const entry of Object.values(feed.entries)) {
      const effective = patchedSession(entry.session, patches)
      if (effective.archived === true) continue
      const approvals = entry.card.approvals
      if (deriveBoardStatus({ ...effective, card: { approvals } }, { now }).status !== 'approval')
        continue
      if (!approvals.length || approvals.some((approval) => approval.truncated))
        ids.add(entry.session.id)
    }
    return ids
  }
  // 第 2 期数据源的第一次请求还没结果：先等 card，不抢着发 /live。
  if (sync.mode !== 'legacy' && !sync.feedSettled) return ids
  for (const session of readBoardSessions()) {
    const effective = patchedSession(session, patches)
    if (effective.archived === true) continue
    // 推导与看板一致：只有“需要你 · 等待批准”的卡片需要 /live 补全审批 id 与完整命令。
    if (deriveBoardStatus(effective, { now }).status === 'approval') ids.add(session.id)
  }
  return ids
}

function dropLiveDetail(sessionId: string) {
  const timer = sync.liveTimers.get(sessionId)
  if (timer) clearTimeout(timer)
  sync.liveTimers.delete(sessionId)
  sync.liveControllers.get(sessionId)?.abort()
  sync.liveControllers.delete(sessionId)
  sync.liveNextAt.delete(sessionId)
  updateLive(sessionId, () => null)
}

export function reconcileLiveDetails() {
  if (!sync.active) return
  const wanted = approvalSessionIds()
  for (const id of Object.keys(useBoardOverlay.getState().live))
    if (!wanted.has(id)) dropLiveDetail(id)
  for (const id of [...sync.liveControllers.keys()]) if (!wanted.has(id)) dropLiveDetail(id)
  // 页面隐藏时不再发起新请求，相当于退避到下一次可见。
  if (!documentVisible()) return
  const now = Date.now()
  const live = useBoardOverlay.getState().live
  const due = [...wanted]
    .filter((id) => !sync.liveControllers.has(id) && (sync.liveNextAt.get(id) ?? 0) <= now)
    // 从未取过详情的卡片优先，其次是最久没刷新的。
    .sort(
      (a, b) =>
        Number(Boolean(live[a])) - Number(Boolean(live[b])) ||
        (sync.liveNextAt.get(a) ?? 0) - (sync.liveNextAt.get(b) ?? 0),
    )
  const slots = Math.max(
    0,
    Math.min(
      BOARD_LIVE_LIMIT - sync.liveControllers.size,
      BOARD_CONNECTION_BUDGET -
        sync.liveControllers.size -
        sync.runControllers.size -
        (sync.events.controller ? 1 : 0),
    ),
  )
  due.slice(0, slots).forEach((id, index) => startLiveFetch(id, index * BOARD_LIVE_STAGGER_MS))
}

function startLiveFetch(sessionId: string, delay: number) {
  const controller = new AbortController()
  sync.liveControllers.set(sessionId, controller)
  const run = async () => {
    sync.liveTimers.delete(sessionId)
    updateLive(sessionId, (entry) => entry ?? emptyLiveEntry('loading'))
    try {
      const detail = await boardApi.getLiveDetail(sessionId, controller.signal)
      if (controller.signal.aborted || sync.liveControllers.get(sessionId) !== controller) return
      sync.liveNextAt.set(sessionId, Date.now() + BOARD_LIVE_REFRESH_MS)
      // 正在提交决定的审批不再从旧快照里冒出来，避免卡片在请求途中闪回。
      const { pending } = useBoardOverlay.getState()
      const approvals = detail.approvals.filter(
        (approval) => !pending[boardActionKey('approve', sessionId, approval.id)],
      )
      updateLive(sessionId, () => ({
        status: 'ready',
        approvals,
        error: detail.error,
        activity: detail.activity,
        startedAt: detail.startedAt,
        fetchedAt: Date.now(),
        failures: 0,
      }))
    } catch {
      if (controller.signal.aborted || sync.liveControllers.get(sessionId) !== controller) return
      const failures = (useBoardOverlay.getState().live[sessionId]?.failures ?? 0) + 1
      sync.liveNextAt.set(
        sessionId,
        Date.now() + Math.min(BOARD_LIVE_REFRESH_MS * 2 ** failures, BOARD_LIVE_MAX_BACKOFF_MS),
      )
      updateLive(sessionId, (entry) => ({
        ...(entry ?? emptyLiveEntry('error')),
        // 已有审批详情时保留可操作内容，只记录失败次数用于退避。
        status: entry?.approvals.length ? entry.status : 'error',
        failures,
      }))
    } finally {
      if (sync.liveControllers.get(sessionId) === controller) sync.liveControllers.delete(sessionId)
    }
  }
  if (delay <= 0) void run()
  else
    sync.liveTimers.set(
      sessionId,
      setTimeout(() => void run(), delay),
    )
}

// —— 本客户端发起的运行：看板持有流，负责中断与清理 ——
export class BoardRunActiveError extends Error {
  readonly code = 'board_run_active'
}

export type BoardStreamOpener = (onEvent: BoardStreamEvent, signal: AbortSignal) => Promise<void>

let runToken = 0

function sameActivity(left: BoardActivity | null, right: BoardActivity | null) {
  if (left === right) return true
  if (!left || !right || left.type !== right.type) return false
  return left.type === 'model'
    ? left.stage === (right as typeof left).stage
    : left.name === (right as typeof left).name && left.summary === (right as typeof left).summary
}

function updateLocalRun(sessionId: string, token: number, update: Partial<BoardLocalRun>) {
  useBoardOverlay.setState((state) => {
    const current = state.localRuns[sessionId]
    if (!current || current.token !== token) return state
    if (
      (update.activity === undefined || sameActivity(current.activity, update.activity)) &&
      (update.runId === undefined || update.runId === current.runId)
    )
      return state
    return { localRuns: { ...state.localRuns, [sessionId]: { ...current, ...update } } }
  })
}

// 只取一行活动需要的事件；文本增量只在阶段变化时写入，避免每帧都触发看板重算。
function applyRunEvent(
  sessionId: string,
  token: number,
  event: string,
  data: Record<string, unknown>,
) {
  if (event === 'run') {
    updateLocalRun(sessionId, token, { runId: typeof data.runId === 'string' ? data.runId : '' })
  } else if (event === 'tool_start') {
    const name = typeof data.name === 'string' ? data.name : ''
    if (name)
      updateLocalRun(sessionId, token, {
        activity: {
          type: 'tool',
          name,
          summary: summarizeToolArgs(name, data.args),
          startedAt: typeof data.startedAt === 'string' ? data.startedAt : new Date().toISOString(),
        },
      })
  } else if (event === 'tool_end' || event === 'thinking_patch' || event === 'thinking_reset') {
    updateLocalRun(sessionId, token, { activity: { type: 'model', stage: 'thinking' } })
  } else if (event === 'text_delta' || event === 'text_patch') {
    updateLocalRun(sessionId, token, { activity: { type: 'model', stage: 'responding' } })
  } else if (event === 'permission_request') {
    // 自己的流先拿到审批：直接放进详情，省一次 /live；列表随即刷新进入“需要你”。
    const approval = parseBoardApproval(data)
    if (approval) restoreBoardApproval(sessionId, approval)
    void invalidateStartupQuery('sessions')
  } else if (event === 'permission_resolved') {
    if (typeof data.id === 'string') removeBoardApproval(sessionId, data.id)
    void invalidateStartupQuery('sessions')
  }
}

// 超出持有上限时释放最早的已受理流；受理前的流不动（它们很快拿到 run 头帧或失败）。
function releaseExcessStreams() {
  let excess = sync.runControllers.size - BOARD_HELD_STREAM_LIMIT
  for (const [sessionId, run] of sync.runControllers) {
    if (excess <= 0) return
    if (!run.accepted) continue
    sync.runControllers.delete(sessionId)
    run.controller.abort()
    excess -= 1
  }
}

export function boardHeldStreamCount() {
  return sync.runControllers.size
}

/**
 * 由看板持有一条运行流：resolve 表示 Runtime 已受理（收到 run 头帧或流正常结束），
 * reject 表示受理前失败。受理后的断流不再上抛——run 在 Runtime 继续，列表刷新会反映真实状态。
 * 同时持有的流不超过 BOARD_HELD_STREAM_LIMIT：新流进来时，最早的已受理流会被释放。
 */
export function ownBoardStream(sessionId: string, kind: BoardRunKind, open: BoardStreamOpener) {
  if (sync.runControllers.has(sessionId)) return Promise.reject(new BoardRunActiveError())
  const controller = new AbortController()
  const token = ++runToken
  const held: HeldRun = { controller, accepted: false }
  sync.runControllers.set(sessionId, held)
  releaseExcessStreams()
  const startedAt = Date.now()
  useBoardOverlay.setState((state) => ({
    localRuns: {
      ...state.localRuns,
      [sessionId]: {
        token,
        kind,
        runId: '',
        startedAt,
        activity: { type: 'model', stage: 'starting' },
      },
    },
  }))
  return new Promise<void>((resolve, reject) => {
    const accept = () => {
      if (held.accepted) return
      held.accepted = true
      releaseExcessStreams()
      resolve()
    }
    const onEvent: BoardStreamEvent = (event, data) => {
      if (controller.signal.aborted) return false
      if (event === 'run') accept()
      applyRunEvent(sessionId, token, event, data)
      return undefined
    }
    void open(onEvent, controller.signal)
      .then(accept, (error: unknown) => {
        if (!held.accepted) reject(error)
      })
      .finally(() => {
        if (sync.runControllers.get(sessionId) === held) sync.runControllers.delete(sessionId)
        useBoardOverlay.setState((state) => {
          if (state.localRuns[sessionId]?.token !== token) return state
          const localRuns = { ...state.localRuns }
          delete localRuns[sessionId]
          return { localRuns }
        })
        if (sync.active) {
          // 事件流在线时由增量反映运行结束；其余数据源补读一次。
          if (sync.mode === 'legacy') void refreshBoardSessions().catch(() => undefined)
          else if (boardFeatures().view && (sync.mode === 'view' || !sync.events.controller))
            void pollViewFresh()
          // 释放的连接让给排队的 /live。
          reconcileLiveDetails()
        }
      })
  })
}

// 测试与热重载使用：回到初始状态，不残留定时器和请求。
export function resetBoardStore() {
  stopBoardSync()
  sync.retainers = 0
  sync.mode = 'legacy'
  sync.events.attempt = 0
  sync.events.blockedUntil = 0
  sync.view.failures = 0
  sync.feedSettled = false
  sync.catalogFetchAt = 0
  useBoardOverlay.setState({ ...EMPTY_OVERLAY, feed: emptyBoardFeed() })
}
