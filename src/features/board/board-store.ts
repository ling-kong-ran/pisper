// 看板数据层。会话目录的唯一权威所有者是 react-query 的 ['sessions','board']（见 startup-queries）；
// 本模块的 zustand 覆盖层只保存服务端列表没有的实时细节：审批详情（/live）、本客户端发起的运行活动、
// 乐观补丁和进行中的动作。第 1 期 Runtime 没有 board.view / board.events 能力，按降级矩阵：
// 页面可见时每 3 秒刷新一次 /api/sessions，隐藏时停止；需要你的审批卡单独取 /live。
// 轮询、/live 请求与流消费只在有视图持有（retainBoardSync）时运行，最后一个持有者释放后统一清理。
import { hashKey } from '@tanstack/react-query'
import { create } from 'zustand'
import { deriveBoardStatus } from '@shared/session-board-status.mjs'
import {
  fetchStartupQuery,
  invalidateStartupQuery,
  queryClient,
  SESSIONS_QUERY_KEY,
  startupQueryOptions,
} from '@/lib/startup-queries'
import type { SessionSummary } from '@/types/chat'
import {
  boardApi,
  parseBoardApproval,
  summarizeToolArgs,
  type BoardActivity,
  type BoardApproval,
  type BoardStreamEvent,
} from './board-api'

export const BOARD_SESSIONS_QUERY_KEY = SESSIONS_QUERY_KEY
export const BOARD_POLL_INTERVAL_MS = 3_000
// 连接预算：Runtime 走 HTTP/1.1，Chromium、WebView2、WKWebView 对同一主机最多 6 条连接。
// 看板长期占用的连接（本客户端持有的运行流 + 在途的 /live）合计不超过 4 条，
// 给 3 秒轮询、审批提交与其他页面请求至少留 2 条，否则连续派发几个长任务后整个应用会排队卡死。
export const BOARD_CONNECTION_BUDGET = 4
// 最多持有 2 条运行流（给出实时活动行）；更早的流在 Runtime 受理后释放，run 在 Runtime 继续，
// 卡片退回降级形态，由列表轮询反映真实状态。
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

export type BoardActionKind = 'approve' | 'deny' | 'retry' | 'markHandled' | 'pin' | 'archive'

export type BoardOverlayState = {
  live: Record<string, BoardLiveEntry>
  localRuns: Record<string, BoardLocalRun>
  // 首次观察到运行的时间：降级模式拿不到真实开始时间，用它保持运行中泳道顺序稳定。
  runningSince: Record<string, number>
  patches: Record<string, Array<{ token: number; patch: BoardSessionPatch }>>
  pending: Record<string, BoardActionKind>
}

const EMPTY_OVERLAY: BoardOverlayState = {
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

export function removeBoardApproval(sessionId: string, approvalId: string) {
  updateLive(sessionId, (entry) =>
    entry && entry.approvals.some((approval) => approval.id === approvalId)
      ? { ...entry, approvals: entry.approvals.filter((approval) => approval.id !== approvalId) }
      : (entry ?? null),
  )
}

export function restoreBoardApproval(sessionId: string, approval: BoardApproval) {
  updateLive(sessionId, (entry) => {
    const base = entry ?? emptyLiveEntry('ready')
    if (base.approvals.some((item) => item.id === approval.id)) return base
    return { ...base, approvals: [...base.approvals, approval] }
  })
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

// —— 同步生命周期：单一轮询者 ——
type SyncRuntime = {
  retainers: number
  active: boolean
  pollTimer: ReturnType<typeof setTimeout> | null
  releaseTimer: ReturnType<typeof setTimeout> | null
  liveTimers: Map<string, ReturnType<typeof setTimeout>>
  liveControllers: Map<string, AbortController>
  liveNextAt: Map<string, number>
  // 按开始顺序排列（Map 保留插入顺序），超出上限时先释放最早的已受理流。
  runControllers: Map<string, HeldRun>
  cleanups: Array<() => void>
}

type HeldRun = { controller: AbortController; accepted: boolean }

const sync: SyncRuntime = {
  retainers: 0,
  active: false,
  pollTimer: null,
  releaseTimer: null,
  liveTimers: new Map(),
  liveControllers: new Map(),
  liveNextAt: new Map(),
  runControllers: new Map(),
  cleanups: [],
}

function documentVisible() {
  return typeof document === 'undefined' || document.visibilityState !== 'hidden'
}

function schedulePoll(delay: number) {
  if (sync.pollTimer) clearTimeout(sync.pollTimer)
  sync.pollTimer = null
  if (!sync.active || !documentVisible()) return
  sync.pollTimer = setTimeout(() => {
    sync.pollTimer = null
    void pollOnce()
  }, delay)
}

async function pollOnce() {
  try {
    await refreshBoardSessions()
  } catch {
    // 失败保留在查询状态里供界面展示；上一份有效快照不被清空，下一轮继续尝试。
  }
  if (!sync.active) return
  reconcileLiveDetails()
  schedulePoll(BOARD_POLL_INTERVAL_MS)
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
  const sessionsHash = hashKey(BOARD_SESSIONS_QUERY_KEY)
  sync.cleanups.push(
    queryClient.getQueryCache().subscribe((event) => {
      if (event.type !== 'updated' || event.query.queryHash !== sessionsHash) return
      if (event.action.type !== 'success') return
      reconcileSessions()
    }),
  )
  if (typeof document !== 'undefined') {
    const visibilityChanged = () => {
      if (documentVisible()) schedulePoll(0)
      else if (sync.pollTimer) {
        clearTimeout(sync.pollTimer)
        sync.pollTimer = null
      }
    }
    document.addEventListener('visibilitychange', visibilityChanged)
    sync.cleanups.push(() => document.removeEventListener('visibilitychange', visibilityChanged))
  }
  reconcileSessions()
  // 有新鲜快照时不必立刻重复请求；陈旧或缺失时马上补读。
  const updatedAt = queryClient.getQueryState(BOARD_SESSIONS_QUERY_KEY)?.dataUpdatedAt ?? 0
  schedulePoll(Math.max(0, updatedAt + BOARD_POLL_INTERVAL_MS - Date.now()))
}

// 停止全部同步工作：轮询、/live 请求与本客户端持有的流消费。Runtime 的 run 不受影响。
export function stopBoardSync() {
  sync.active = false
  if (sync.pollTimer) clearTimeout(sync.pollTimer)
  if (sync.releaseTimer) clearTimeout(sync.releaseTimer)
  sync.pollTimer = null
  sync.releaseTimer = null
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

// 列表变化后的覆盖层整理：记录运行起点、清掉已消失会话的细节，并为新出现的审批卡安排 /live。
function reconcileSessions() {
  const sessions = readBoardSessions()
  const now = Date.now()
  const { runningSince, localRuns } = useBoardOverlay.getState()
  const next: Record<string, number> = {}
  let changed = false
  for (const session of sessions) {
    if (session.streaming !== true) continue
    next[session.id] = runningSince[session.id] ?? localRuns[session.id]?.startedAt ?? now
    if (!(session.id in runningSince)) changed = true
  }
  if (!changed) changed = Object.keys(runningSince).some((id) => !(id in next))
  if (changed) useBoardOverlay.setState({ runningSince: next })
  reconcileLiveDetails()
}

function approvalSessionIds() {
  const { patches } = useBoardOverlay.getState()
  const ids = new Set<string>()
  for (const session of readBoardSessions()) {
    const effective = patchedSession(session, patches)
    if (effective.archived === true) continue
    // 推导与看板一致：只有“需要你 · 等待批准”的卡片需要 /live 补全审批 id 与完整命令。
    if (deriveBoardStatus(effective, { now: Date.now() }).status === 'approval') ids.add(session.id)
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
      BOARD_CONNECTION_BUDGET - sync.liveControllers.size - sync.runControllers.size,
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
          void refreshBoardSessions().catch(() => undefined)
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
  useBoardOverlay.setState({ ...EMPTY_OVERLAY })
}
