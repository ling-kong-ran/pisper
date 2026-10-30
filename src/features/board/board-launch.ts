// 派发新任务：与聊天页“创建会话并发送首条消息”走同一套接口——先 POST /api/sessions 建会话，
// 按启动台芯片写入模型、思考档位、审批模式与运行模式，再经 /api/chat 发送首条消息。
// 运行流交给看板 store 持有（ownBoardStream）：Runtime 受理后即返回，启动台可以马上派发下一个；
// 看板卸载或同步停止只会断开本客户端的 SSE，Runtime 的运行不受影响，列表轮询会继续反映真实状态。
// 建好会话之后、首条消息被受理之前失败（例如 409 能力不支持）时，确认会话仍是空的再删掉它，
// 不在历史会话里留下没有消息的空壳。
import { chatApi } from '@/features/chat/chat-api'
import { announceSessionsUpdated } from '@/features/chat/events'
import { ApiError, requestJson } from '@/lib/http'
import { invalidateStartupQuery, queryClient } from '@/lib/startup-queries'
import type { SessionSummary } from '@/types/chat'
import {
  applyBoardPatch,
  BOARD_SESSIONS_QUERY_KEY,
  clearBoardPatch,
  confirmBoardSession,
  confirmBoardSessions,
  ownBoardStream,
  type BoardSessionsData,
} from './board-store'
import type { LaunchExecutionMode, LaunchRunMode } from './launch-defaults-store'

export type BoardLaunchRequest = {
  prompt: string
  cwd: string
  // provider/modelId；空串表示沿用 Runtime 默认模型。
  model: string
  thinkingLevel: string
  executionMode: LaunchExecutionMode | ''
  runMode: LaunchRunMode
}

export type BoardLaunchStage = 'create' | 'configure' | 'send'

export class BoardLaunchError extends Error {
  readonly code = 'board_launch_failed'
  constructor(
    readonly stage: BoardLaunchStage,
    readonly cause: unknown,
    readonly sessionId = '',
  ) {
    super(cause instanceof Error ? cause.message : String(cause || 'launch failed'))
  }
}

const MAX_PROMPT_CHARS = 100_000
// 清理是尽力而为：两次请求都设短超时，Runtime 迟迟不应答时宁可保留空会话，也不拖住错误提示。
const CLEANUP_TIMEOUT_MS = 5_000

// /live 的快照按不可信数据检查：只有明确“没有任何消息、没有排队输入、没有在运行或配置”才算空会话。
export function isUnsentLaunchSession(live: unknown) {
  if (!live || typeof live !== 'object') return false
  const record = live as Record<string, unknown>
  return (
    Array.isArray(record.messages) &&
    record.messages.length === 0 &&
    record.streaming === false &&
    record.configurationBusy !== true &&
    (record.queuedInputs === undefined ||
      (Array.isArray(record.queuedInputs) && record.queuedInputs.length === 0))
  )
}

// 首条消息被 Runtime 明确拒绝（run 头帧之前的 HTTP 错误响应）时才可能是空会话；
// 断网、超时、主动中止都说不清 Runtime 是否已经受理，不碰会话。写配置阶段从未发送消息，总会检查。
function sendRejected(error: unknown) {
  return error instanceof ApiError && error.kind === 'http'
}

// 日志只写错误类别与 HTTP 状态：不写会话 id、路径、提示词或错误正文（可能含用户内容）。
function cleanupFailureCode(error: unknown) {
  return error instanceof ApiError ? `${error.kind || 'error'}:${error.status ?? '-'}` : 'error'
}

/**
 * 派发失败后清理刚建的空会话；只有 /live 确认它没有消息、没有排队、也不在运行时才删除。
 * 返回是否删除。删除失败静默保留会话，只记一条不含路径与用户内容的日志，从不抛错。
 */
export async function discardUnsentLaunchSession(sessionId: string) {
  const path = `/api/sessions/${encodeURIComponent(sessionId)}`
  try {
    const live = await chatApi.getLiveSession(sessionId, { timeout: CLEANUP_TIMEOUT_MS })
    if (!isUnsentLaunchSession(live)) return false
    await requestJson(path, { method: 'DELETE', timeout: CLEANUP_TIMEOUT_MS })
  } catch (error) {
    console.warn(`[board] 派发失败后未能清理空会话（${cleanupFailureCode(error)}）`)
    return false
  }
  queryClient.setQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY, (current) =>
    current
      ? { ...current, sessions: current.sessions.filter((session) => session.id !== sessionId) }
      : current,
  )
  announceSessionsUpdated({ deletedIds: [sessionId] })
  return true
}

function mergeCreatedSession(created: SessionSummary) {
  // 新卡片立即进入权威缓存，不等下一轮轮询；随后的确认刷新以服务端快照为准。
  queryClient.setQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY, (current) =>
    current && !current.sessions.some((session) => session.id === created.id)
      ? { ...current, sessions: [created, ...current.sessions] }
      : current,
  )
  // 推进版本：合并之前发出的旧轮询快照作废并补读，不会把刚出现的新卡片短暂抹掉。
  void invalidateStartupQuery('sessions')
}

function splitModel(model: string) {
  const index = model.indexOf('/')
  return index > 0 ? { provider: model.slice(0, index), modelId: model.slice(index + 1) } : null
}

async function configure(sessionId: string, created: SessionSummary, request: BoardLaunchRequest) {
  // 只写与新会话默认值不同的选择，避免无谓的配置请求触发会话重载。
  const target = splitModel(request.model)
  if (target && request.model !== created.model)
    await chatApi.updateModel(sessionId, target.provider, target.modelId)
  if (request.thinkingLevel && request.thinkingLevel !== created.thinkingLevel)
    await chatApi.setThinkingLevel(sessionId, request.thinkingLevel)
  if (request.executionMode && request.executionMode !== created.executionMode)
    await chatApi.updateExecutionMode(sessionId, request.executionMode)
  if (request.runMode !== 'plan' && request.runMode !== created.runMode)
    await chatApi.updateRunMode(sessionId, request.runMode)
}

/**
 * 派发一个新任务。resolve 返回新会话 id，表示 Runtime 已受理首条消息；
 * reject 为 BoardLaunchError，stage 指出失败发生在哪一步（建会话 / 写配置 / 发送）。
 */
export async function launchBoardTask(request: BoardLaunchRequest): Promise<string> {
  const prompt = request.prompt.trim().slice(0, MAX_PROMPT_CHARS)
  if (!prompt) throw new BoardLaunchError('create', new Error('empty prompt'))
  let created: SessionSummary
  try {
    created = await chatApi.createSession('', request.cwd)
  } catch (error) {
    throw new BoardLaunchError('create', error)
  }
  const sessionId = created.id
  try {
    await configure(sessionId, created, request)
  } catch (error) {
    await discardUnsentLaunchSession(sessionId)
    throw new BoardLaunchError('configure', error, sessionId)
  }
  mergeCreatedSession({ ...created, firstMessage: prompt, messageCount: 1 })
  // 首轮受理前就让卡片出现在“运行中”；失败时撤掉补丁，空会话不上看板。
  const token = applyBoardPatch(sessionId, { streaming: true })
  try {
    await ownBoardStream(sessionId, 'launch', (onEvent, signal) =>
      chatApi.openStream(
        {
          sessionId,
          message: prompt,
          attachments: [],
          goalMode: request.runMode === 'goal',
          teamMode: request.runMode === 'team',
        },
        onEvent,
        { signal },
      ),
    )
  } catch (error) {
    clearBoardPatch(sessionId, token)
    if (sendRejected(error)) await discardUnsentLaunchSession(sessionId)
    void confirmBoardSessions().catch(() => undefined)
    throw new BoardLaunchError('send', error, sessionId)
  }
  // 事件流在线时等这张新卡片的第一帧（条目接管后补丁才撤），其余数据源立即补读。
  void confirmBoardSession(sessionId, (entry) => Boolean(entry))
    .catch(() => undefined)
    .finally(() => clearBoardPatch(sessionId, token))
  return sessionId
}
