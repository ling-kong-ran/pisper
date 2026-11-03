// 新任务与已有会话的任务消息：新任务先 POST /api/sessions 建会话，按草稿的芯片写入模型、思考档位、
// 审批模式与运行模式（prepareLaunchSession），第一条消息由会话页自己发出并持有运行流；
// 给已有会话发任务消息（组件工作室）时运行流交给看板 store 持有（ownBoardStream）。
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
  BoardRunActiveError,
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
  // 不选项目：忽略 cwd，Runtime 为这个会话新建空文件夹。
  projectless?: boolean
  // provider/modelId；空串表示沿用 Runtime 默认模型。
  model: string
  thinkingLevel: string
  executionMode: LaunchExecutionMode | ''
  runMode: LaunchRunMode
  // 排在这个会话之后启动（A9，能力 sessionQueue）；不给表示立即开始。
  startAfter?: string
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

export type PreparedLaunchSession = {
  // 建好并写好配置的新会话（排队时带 Runtime 给的 queue）。
  session: SessionSummary
  // true：首条消息已登记进启动队列，等前一个会话这一轮结束由 Runtime 启动，调用方不再发送。
  queued: boolean
}

/**
 * 新任务的第一条消息发出前：建会话，按草稿的选择写入模型、思考档位、审批模式与运行模式；
 * 选了“排在…之后”时把首条消息登记进启动队列。不发送消息、也不持有运行流——
 * 会话页（新任务草稿）随后用自己的输入框发出第一条消息，回复在同一页流式出现。
 * reject 为 BoardLaunchError；写配置或登记排队失败时已清理空会话。
 */
export async function prepareLaunchSession(
  request: BoardLaunchRequest,
): Promise<PreparedLaunchSession> {
  const prompt = request.prompt.trim().slice(0, MAX_PROMPT_CHARS)
  if (!prompt) throw new BoardLaunchError('create', new Error('empty prompt'))
  let created: SessionSummary
  try {
    created = await chatApi.createSession('', request.cwd, {
      projectless: request.projectless === true,
    })
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
  if (request.startAfter) {
    const session = await queueLaunchedSession(sessionId, created, prompt, request)
    return { session, queued: true }
  }
  return { session: created, queued: false }
}

/**
 * 排在 X 之后（规格 8.4）：首条消息交给 Runtime 的启动队列，不在这里发送，也不由看板持有运行流——
 * 等 X 这一轮结束时 Runtime 在后台启动它。X 已经结束时 Runtime 直接启动（started）。
 * 登记失败按“发送失败”处理：清理空会话、抛 BoardLaunchError('send')。
 */
async function queueLaunchedSession(
  sessionId: string,
  created: SessionSummary,
  prompt: string,
  request: BoardLaunchRequest,
) {
  let result: Record<string, unknown>
  try {
    result = await chatApi.enqueueSessionStart(sessionId, {
      startAfter: request.startAfter ?? '',
      message: prompt,
      runMode: request.runMode,
    })
  } catch (error) {
    if (sendRejected(error)) await discardUnsentLaunchSession(sessionId)
    void confirmBoardSessions().catch(() => undefined)
    throw new BoardLaunchError('send', error, sessionId)
  }
  const queue = result && typeof result.queue === 'object' ? result.queue : null
  // 排队的会话还没有消息：带上 Runtime 给的 queue 并进目录，卡片马上出现在“排队”；直接启动的进“运行中”。
  const session: SessionSummary = {
    ...created,
    firstMessage: prompt,
    ...(queue ? { queue } : { messageCount: 1, streaming: true }),
  }
  mergeCreatedSession(session)
  void confirmBoardSession(sessionId, (entry) => Boolean(entry)).catch(() => undefined)
  return session
}

export type BoardTaskMessage = {
  sessionId: string
  prompt: string
  // 会话还没有任何消息时放在提示词前的命令（例如 '/skill:pisper-component'）。命令与提示词之间必须是空格：
  // Pi 只按第一个空格切出技能名，换行会让后面的提示词也被当成技能名的一部分。
  commandWhenEmpty?: string
  // 调用方刚建好的会话摘要（与 POST /api/sessions 同形）：并入权威缓存，卡片不等下一轮轮询就出现在“运行中”。
  created?: SessionSummary | null
}

export type BoardTaskMessageResult = { sessionId: string; queued: boolean }

const SKILL_COMMAND = /^\/[a-z][a-z0-9:-]{0,63}$/

/**
 * 给一个已有会话发一条任务消息（计划 §4.1 / §5.4：组件工作室的“生成”与“让代理修复”）。
 * 与派发新任务同一套流所有权：会话空闲时由看板 store 持有运行流（ownBoardStream），Runtime 受理即返回，
 * 卡片随之进入“运行中”；会话正在运行（或本客户端已持有它的流）时排在当前运行之后（followUp），不打断它。
 * reject 为 BoardLaunchError（stage 'send'）；不会删除会话——它是调用方的，不是这里新建的。
 */
export async function sendBoardTaskMessage(
  input: BoardTaskMessage,
): Promise<BoardTaskMessageResult> {
  const { sessionId } = input
  const prompt = input.prompt.trim().slice(0, MAX_PROMPT_CHARS)
  if (!sessionId || !prompt)
    throw new BoardLaunchError('send', new Error('empty prompt'), sessionId)
  const command = input.commandWhenEmpty?.trim() ?? ''
  if (command && !SKILL_COMMAND.test(command))
    throw new BoardLaunchError('send', new Error('invalid command'), sessionId)
  let live: unknown
  try {
    live = await chatApi.getLiveSession(sessionId)
  } catch (error) {
    throw new BoardLaunchError('send', error, sessionId)
  }
  const record = live && typeof live === 'object' ? (live as Record<string, unknown>) : {}
  const empty = Array.isArray(record.messages) && record.messages.length === 0
  const message = empty && command ? `${command} ${prompt}` : prompt
  const queue = async () => {
    try {
      await chatApi.queueInput(sessionId, message, [], 'followUp')
    } catch (error) {
      throw new BoardLaunchError('send', error, sessionId)
    }
    return { sessionId, queued: true }
  }
  if (record.streaming === true) return queue()
  if (input.created?.id === sessionId)
    mergeCreatedSession({ ...input.created, firstMessage: prompt, messageCount: 1 })
  else if (
    !queryClient
      .getQueryData<BoardSessionsData>(BOARD_SESSIONS_QUERY_KEY)
      ?.sessions.some((session) => session.id === sessionId)
  )
    // 缓存里还没有这个会话（例如在别的设备上建的）：补读一次，补丁与本地运行等它出现后生效。
    void invalidateStartupQuery('sessions')
  const token = applyBoardPatch(sessionId, { streaming: true })
  try {
    await ownBoardStream(sessionId, 'launch', (onEvent, signal) =>
      chatApi.openStream(
        { sessionId, message, attachments: [], goalMode: false, teamMode: false },
        onEvent,
        { signal },
      ),
    )
  } catch (error) {
    clearBoardPatch(sessionId, token)
    // 本客户端已持有这条会话的流（上一条还没跑完）：排在后面，与聊天页运行中发送一致。
    if (error instanceof BoardRunActiveError) return queue()
    void confirmBoardSessions().catch(() => undefined)
    throw new BoardLaunchError('send', error, sessionId)
  }
  void confirmBoardSession(sessionId, (entry) => Boolean(entry))
    .catch(() => undefined)
    .finally(() => clearBoardPatch(sessionId, token))
  return { sessionId, queued: false }
}
