// 实时会话同步辅助：判断当前状态里是否有仍在运行的 Agent，
// 供 UI 决定是否显示“正在运行”标记与保留关闭后的会话状态。
import type { SessionState } from '@/types/chat'

// 当前是否有活跃 agent（排队/启动/运行中），供 UI 决定运行标记与轮询策略。
export function hasActiveSessionAgents(state: Partial<SessionState> | undefined) {
  return Boolean(
    state?.agents?.some((agent) =>
      ['queued', 'starting', 'running'].includes(String(agent.status)),
    ),
  )
}

// 是否还需要轮询实时状态：本地已持有流（localStreamOwned）时不轮询；
// 否则在 恢复中/流式中/有待审批/有活跃 agent 时轮询补数据。
export function shouldPollLiveSession(
  state: Partial<SessionState> | undefined,
  { localStreamOwned = false }: { localStreamOwned?: boolean } = {},
) {
  if (!state || localStreamOwned) return false
  return Boolean(
    state.recovering || state.streaming || state.approvals?.length || hasActiveSessionAgents(state),
  )
}

// 轮询每 2 秒走一拍。这一轮在跑（或在恢复）时每拍都读；只是挂着批准、或有后台的活在跑而对话本身空闲时
// 每 3 拍读一次：这段时间可能很长，整页快照没必要 2 秒拉一次（Bot 的活另有自己的轮询）。
export const IDLE_LIVE_POLL_EVERY = 3

export function liveSessionPollEvery(state: Partial<SessionState> | undefined) {
  return state?.recovering || state?.streaming ? 1 : IDLE_LIVE_POLL_EVERY
}

export const LOCAL_STREAM_STALE_MS = 10_000

// 回到前台 / 恢复联网时，要不要让本页自己持有的流作废、改用服务端快照。
// 移动 WebView 和手机浏览器挂起后，连接可能表面活着却不再出数据，一律作废；断网恢复、从往返缓存恢复时
// 连接已经断了，也作废。桌面上只是切了一下标签页时流照常在收：最近还有事件就留着它，
// 否则这一轮剩下的部分会降成 2 秒一跳的整页轮询，完成通知也发不出来。
export function shouldReclaimLocalStream({
  mobile,
  reason,
  lastEventAt,
  now = Date.now(),
}: {
  mobile: boolean
  reason: 'visible' | 'online' | 'restored'
  lastEventAt: number
  now?: number
}) {
  if (mobile || reason !== 'visible') return true
  return !lastEventAt || now - lastEventAt >= LOCAL_STREAM_STALE_MS
}
