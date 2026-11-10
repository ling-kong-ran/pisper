// AgentRuntimeFacade：AgentRuntimeService 的“门面”基类。
// 把各领域服务（记忆/目标/计划/多 Agent/渠道/工作流/定时/插件/MCP/技能/Provider 等）
// 的调用组织成 HTTP API 层可直接调用的方法集合；子类 AgentRuntimeService 在
// 构造器里完成依赖装配后，这些方法即成为对外接口。
import { randomUUID } from 'node:crypto'
import { mkdir, readFile } from 'node:fs/promises'
import { isAbsolute, join, relative, resolve } from 'node:path'
import { WorkspaceAssetTracker } from '../services/workspace-asset-tracker.mjs'
import { SessionFileChangesService } from '../services/session-file-changes.mjs'
import { TurnCheckpointService } from '../services/turn-checkpoints.mjs'
import {
  SESSION_MESSAGE_LIMIT,
  WorkspaceCoordination,
  foldersOverlap,
  sessionMessageContent,
} from '../services/workspace-coordination.mjs'
import { loadAgentProfiles } from '../services/agent-profiles.mjs'
import { revealLocalPath } from '../services/local-path-service.mjs'
import { projectApproval } from '../services/session-card-summary.mjs'
import {
  botApprovalText,
  botQuestionText,
  botReplyText,
  botChannelActionReply,
  botNotifyText,
} from '../services/bots/bot-notifications.mjs'
import { botNameKey, normalizeBotMessage } from '../services/bots/bot-model.mjs'
import { botMessageContext, isCourtesyMessage, newBotChainId } from '../services/bots/bot-links.mjs'
import {
  BOT_MAX_PARALLEL_AGENTS,
  botReviewInstruction,
  botStateSnapshot,
  normalizeBotAction,
  parseBotSkillReview,
  resolveBotAgent,
} from '../services/bots/bot-conversation.mjs'
import { botTurnTrigger, requestBotText } from './bot-decisions.mjs'
import { botConversationApprovalRefusal } from './tool-activation.mjs'
import { normalizeQuestions } from '../services/session-question-service.mjs'
import { botAnswerText, botScheduleInput } from '../services/bots/bot-schedule-input.mjs'
import { botSourceTrust, stricterBotTrust } from '../services/bots/bot-memory-service.mjs'
import { botSkillsContext } from '../services/bots/bot-skill-service.mjs'
import {
  BOT_PROACTIVE_NOTE,
  BOT_PROACTIVE_TASK_NAME,
  botBriefPrompt,
  botProactiveDue,
  botProactiveTask,
  botProactiveTools,
} from '../services/bots/bot-proactive.mjs'
import { agentCompletionPrompt } from '../services/multi-agent-service.mjs'
import { ExecutorError, NodeExecutor } from '../services/bots/executor.mjs'
import { EnvironmentError, pathWithinRoots } from '../services/bots/environment-service.mjs'
import { isSshMachineId } from '../services/ssh-machine-service.mjs'
import { listWorkspaceFolders } from '../services/workspace-folder-service.mjs'
import { projectQuestionSummary } from '../services/session-question-service.mjs'
import { bridgePiExtensionPackages, isPiExtensionPlugin } from '../services/pi-extension-bridge.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { TOOL_PRESETS, toolsFromConfig } from '../tools/registry.mjs'
import { projectSessionCommands } from './session-commands.mjs'
import { resolveWorkspaceDirectory } from './workspace-directories.mjs'
import { normalizeSideQuestion, streamSideQuestion } from './side-question.mjs'
import { localDayKey } from './conversation-memory-capture.mjs'
import { finishRunStartTiming, markRunStage, withRunStartTiming } from './run-start-timing.mjs'
import { ATTACHMENT_MARKER, isInternalParentMessage } from './stream-projection.mjs'
import { projectAgentTranscript } from './agent-transcript.mjs'
import { sessionInputQueueSnapshot } from './session-input-queue.mjs'
import {
  MAX_COMPACTION_THRESHOLD_PERCENT,
  MIN_COMPACTION_THRESHOLD_PERCENT,
  normalizeCompactionThresholdPercent,
} from './compaction-policy.mjs'
import {
  applyBetaFlagPatch,
  isBetaFeatureActive,
  normalizeStoredBetaFlags,
  projectBetaFeatures,
} from '../../shared/beta-features.mjs'
import { pisperError } from '../../shared/errors.mjs'

// 隔离上下文模式（工作流/渠道的独立子任务）下屏蔽的记忆相关工具：
// 子任务不应读写主会话的长期记忆。
export const ISOLATED_CONTEXT_BLOCKED_TOOLS = [
  'memory_search',
  'memory_remember',
  'memory_open',
  'memory_confirm',
  'memory_forget',
]

// 准备阶段在 /live 里投影的待发消息上限（只用于显示，完整消息照常写进会话）。
const MAX_PREPARING_MESSAGE_CHARS = 20_000
// 每个 Bot 每小时最多发几条 bot_notify。
const BOT_NOTIFY_LIMIT = 10
const BOT_NOTIFY_WINDOW_MS = 60 * 60_000
const CHANNEL_NAMES = { qq: 'QQ', telegram: 'Telegram', feishu: 'Feishu', weixin: 'WeChat' }

// 已启用的通知渠道集合：只有完成连接或授权的渠道才允许工作流与定时任务选择。
export function enabledNotificationTargets(notificationSettings) {
  return new Set([
    ...(notificationSettings.browser?.enabled ? ['browser'] : []),
    ...(notificationSettings.connections?.feishu?.enabled ? ['feishu'] : []),
    ...(notificationSettings.connections?.weixin?.enabled ? ['weixin'] : []),
    ...(notificationSettings.connections?.qq?.enabled ? ['qq'] : []),
    ...(notificationSettings.connections?.telegram?.enabled ? ['telegram'] : []),
  ])
}

// 定时任务 → 工作流适配器：把 WorkflowService 暴露成 ScheduleService 需要的接口。
export function createScheduleWorkflowAdapter(workflows) {
  return {
    list: () => workflows.getState().workflows,
    run: (id, options) => workflows.runNow(id, options),
    getRun: (id) => workflows.getRun(id),
  }
}

// 工作流通知目标过滤：剔除未启用的通知渠道，防止配置了但没连通的渠道在运行时静默失败。
export function filterWorkflowNotificationTargets(input, enabledTargets) {
  if (!input || typeof input !== 'object') return input
  const filtered = { ...input }
  // 部分 PATCH 未提供的字段保持缺席，不能用 undefined 覆盖已有图和通知配置。
  if (Array.isArray(input.notifications))
    filtered.notifications = input.notifications.filter((target) => enabledTargets.has(target))
  if (Array.isArray(input.nodes))
    filtered.nodes = input.nodes.map((node) =>
      node && typeof node === 'object' && Array.isArray(node.notificationTargets)
        ? {
            ...node,
            notificationTargets: node.notificationTargets.filter((target) =>
              enabledTargets.has(target),
            ),
          }
        : node,
    )
  return filtered
}

export class AgentRuntimeFacade {
  async revealLocalPath(path) {
    if (this.capabilities?.profile !== 'desktop')
      throw pisperError('unsupported', 'folder', 'reveal in file manager')
    return await revealLocalPath(path)
  }

  // 旁问（/btw）：读当前会话的上下文快照发一次无头请求，不改主会话，也不登记为可重挂的 run。
  // onStart 在校验通过、即将请求模型时调用：此前的错误按普通请求错误返回。
  async askSideQuestion(sessionId, question, { signal, onStart, onDelta } = {}) {
    const text = normalizeSideQuestion(question)
    const current = await this.getExistingSession(String(sessionId || ''))
    if (!current?.session) throw pisperError('not_found', 'session')
    onStart?.()
    const result = await streamSideQuestion({
      modelRuntime: this.modelRuntime,
      session: current.session,
      question: text,
      signal,
      onDelta,
    })
    // 旁问不进会话文件，用量扫描看不到它；单独记一条，统计里的花费才完整。
    if (result.usage) {
      await this.recordUsage(
        localDayKey(),
        `side-question:${sessionId}:${randomUUID()}`,
        result.usage,
      ).catch(() => {})
    }
    return result
  }

  async getSessionCommands(sessionId) {
    return this.commandsForDirectory(await this.sessionWorkspaceCwd(sessionId), sessionId)
  }

  // 新任务页还没有会话：按所选项目目录列出同一套 Prompt / Skill 命令，目录校验与建会话一致。
  async getWorkspaceCommands(cwd) {
    return this.commandsForDirectory(await resolveWorkspaceDirectory(cwd, this.cwd), '')
  }

  async commandsForDirectory(cwd, sessionId) {
    const loader = await this.skills.createResourceLoader(cwd)
    const prompts = loader.getPrompts()
    const skills = loader.getSkills()
    return projectSessionCommands({
      sessionId,
      prompts: prompts.prompts,
      skills: skills.skills.filter((skill) => !this.skills.isBuiltinSkill?.(skill)),
      diagnostics: prompts.diagnostics,
    })
  }

  deriveSession(id, boundaryEntryId, name) {
    return this.sessionLifecycle.deriveSession(id, boundaryEntryId, name)
  }

  getSessionTree(id) {
    return this.sessionLifecycle.getSessionTree(id)
  }

  searchSessionTreeLabels(query, options) {
    return this.sessionLifecycle.searchSessionTreeLabels(query, options)
  }

  navigateSessionTree(id, targetEntryId, options) {
    return this.sessionLifecycle.navigateSessionTree(id, targetEntryId, options)
  }

  prepareLastTurnRetry(id) {
    return this.sessionLifecycle.prepareLastTurnRetry(id)
  }

  setSessionTreeLabel(id, targetEntryId, label) {
    return this.sessionLifecycle.setSessionTreeLabel(id, targetEntryId, label)
  }

  // 归档附件：上传类附件落盘为资产；路径引用型附件跳过（只保留引用）。
  // 归档失败不阻塞聊天（best-effort）。
  async archiveAttachments(sessionId, sessionName, attachments = []) {
    const archived = []
    for (const attachment of attachments) {
      if (attachment.kind === 'path') {
        archived.push(null)
        continue
      }
      try {
        const created = await this.createAsset({
          name: attachment.name,
          mimeType: attachment.mimeType,
          data: attachment.data,
          text: attachment.kind === 'text' ? attachment.text : undefined,
          source: 'attachment',
          sessionId,
          sessionName,
        })
        const stored = this.assetIndex.assets.find((asset) => asset.id === created.id)
        archived.push(
          stored ? { id: stored.id, path: stored.storagePath || stored.filePath || '' } : null,
        )
      } catch {
        // Asset archival must not block the chat request.
        archived.push(null)
      }
    }
    return archived
  }

  resolveDefaultModel() {
    return this.providerPreferences.resolveDefaultModel()
  }

  getMemoryDashboard(input) {
    return this.memory.getDashboard(input)
  }

  // 候选箱已取消，记忆直接生效。旧版前端（手机 App 的内嵌页面）还会请求这几个接口，给空结果。
  getMemoryCandidateInbox() {
    return { count: 0, candidates: [] }
  }

  getMemoryEvolution(input) {
    return this.memory.getEvolution(input)
  }

  listMemoryEvents(input) {
    return this.memory.listEvents(input)
  }

  async listMemorySessionDigests(input) {
    const digests = this.memory.listSessionDigests(input)
    if (!digests.length) return digests
    // 标题在会话目录里，记忆库不存。目录里已经没有的会话只显示摘要，不给跳转。
    const sessions = new Map((await this.listSessions()).map((session) => [session.id, session]))
    return digests.map((digest) => {
      const session = sessions.get(digest.sessionId)
      return { ...digest, sessionName: session?.name || '', sessionAvailable: Boolean(session) }
    })
  }

  undoMemoryEvent(id) {
    return this.memory.undoEvent(id)
  }

  consolidateMemory() {
    return this.memory.consolidate({ force: true })
  }

  clearMemoryEpisodes(input) {
    return { cleared: this.memory.clearEpisodes(input) }
  }

  exportMemory(input) {
    return this.memory.exportMarkdown(input)
  }

  getMemoryDetail(id) {
    const detail = this.memory.openMemory(id)
    if (!detail) return null
    return { ...detail, history: this.memory.getMemoryHistory(id)?.versions || [] }
  }

  restoreMemory(id) {
    return this.memory.restoreMemory(id)
  }

  confirmMemory(id) {
    return this.memory.confirmMemory(id)
  }

  setMemoryPinned(id, pinned) {
    return this.memory.setPinned(id, pinned)
  }

  createMemorySpace(input) {
    return this.memory.createSpace(input)
  }

  updateMemorySpace(id, input) {
    return this.memory.updateSpace(id, input)
  }

  deleteMemorySpace(id) {
    return this.memory.deleteSpace(id)
  }

  createMemory(input) {
    return this.memory.remember({ ...input, sourceType: 'manual', ignoreForgotten: true })
  }

  updateMemory(id, input) {
    return this.memory.updateMemory(id, input)
  }

  deleteMemory(id) {
    return this.memory.forget(id)
  }

  acceptMemoryCandidate() {
    return null
  }

  rejectMemoryCandidate() {
    return null
  }

  rejectAllMemoryCandidates() {
    return { rejected: 0 }
  }

  async abortSession(id) {
    return this.sessionLifecycle.abortSession(id)
  }

  async deleteSession(id) {
    // 仅在会话生命周期删除成功后清理快照，避免删除失败时丢失可恢复基线。
    const remove = async () => {
      const result = await this.sessionLifecycle.deleteSession(id)
      await this.getFileChangesService().clear(id)
      await this.getTurnCheckpoints().clear(id)
      if (result) {
        await this.workspaceCoordination?.forget(id).catch(() => {})
        // 删除后丢掉内存摘要，并让看板事件流对在线客户端发 removed。
        this.sessionCards?.forget(id, { deleted: true })
        this.boardEvents?.touch(id)
        this.boardEvents?.catalogChanged()
        // 删掉的会话不再排队；排在它后面的会话视为前置已结束，照常启动。
        void this.sessionQueue?.onSessionDeleted(id).catch(() => {})
        this.forgetSessionEpisodes(id)
      }
      return result
    }
    return this.sideChats ? this.sideChats.deleteWithChildren(id, remove) : remove()
  }

  // 会话的原文（L0）跟着会话一起删。手机冷启动时记忆可能还没好，等好了再删；
  // 这一步失败不能影响删除会话本身。
  forgetSessionEpisodes(id) {
    const forget = () => {
      try {
        this.memory.forgetSession?.(id)
      } catch {
        console.warn('[memory] Could not clear the session transcript.')
      }
    }
    const state = this.initialization?.memory
    if (state === 'ready') forget()
    else if (state === 'pending' || state === 'initializing') {
      void Promise.resolve(this.waitForInitialization?.('memory')).then(
        () => {
          if (this.initialization?.memory === 'ready') forget()
        },
        () => {},
      )
    }
  }

  installWorkspaceAssetCapture(session, cwd, sessionId = session.sessionId) {
    this.workspaceAssetTracker ||= new WorkspaceAssetTracker({
      dataDir: this.dataDir,
      archive: (id, path, options) =>
        this.recordGeneratedFile(
          id,
          this.sessions.get(id) || { name: this.sessionMeta[id]?.name },
          path,
          { ...options, deferSave: true },
        ),
      commit: () => this.flushGeneratedAssets(),
    })
    this.workspaceAssetTracker.install(session, { sessionId, cwd })
    // 会话之间的读写台账：装在文件快照之前，被拒绝的写入不记快照。远程会话的文件在别的电脑上，不记。
    if (!this.sessions.get(sessionId)?.remoteExecutor)
      this.getWorkspaceCoordination().install(session, { sessionId, cwd })
    // 同一安装点顺手挂上文件变更追踪：子会话也归属父会话 ID，审批视图自动聚合。
    this.fileChanges ||= new SessionFileChangesService({ dataDir: this.dataDir })
    this.fileChanges.install(session, { sessionId, cwd })
    // 按轮回退文件的快照也挂在这里：子 Agent 的写入同样记在父会话下。
    this.getTurnCheckpoints().install(session, { sessionId, cwd })
  }

  getWorkspaceCoordination() {
    this.workspaceCoordination ||= new WorkspaceCoordination({
      dataDir: this.dataDir,
      describeSession: (id) => this.coordinationSession(id),
      sessionsNear: (file) =>
        Object.entries(this.sessionMeta || {})
          .filter(([, meta]) => meta?.cwd && foldersOverlap(meta.cwd, file))
          .map(([id]) => id),
      // 台账变了：相关会话页的“谁在这里改文件”和留言要重新投影，开着的会话页经看板事件流重读。
      onChange: (ids) => {
        for (const id of ids) {
          this.streamProjection?.invalidate(id, { transcript: false, activity: true, usage: false })
          this.boardEvents?.sessionWorkspaceChanged(id)
        }
      },
      warn: () => console.warn('Could not update the session coordination ledger.'),
    })
    return this.workspaceCoordination
  }

  coordinationSession(id) {
    const value = this.sessions.get(id)
    const meta = this.sessionMeta?.[id]
    if (!value && !meta) return null
    return {
      title: value?.name || meta?.name || '',
      running: Boolean(this.sessionRunIsActive?.(id, value)),
    }
  }

  // 同一个文件夹（目录有包含关系）里的其他会话，最近活动的在前；侧聊不算。
  async workspaceSessions(sessionId) {
    const cwd = this.sessions.get(sessionId)?.cwd || this.sessionMeta?.[sessionId]?.cwd || ''
    if (!cwd) return []
    return (await this.listSessions())
      .filter((item) => item.id !== sessionId && foldersOverlap(item.cwd, cwd))
      .sort((left, right) => String(right.modified).localeCompare(String(left.modified)))
  }

  async listWorkspaceSessions(sessionId) {
    const coordination = this.getWorkspaceCoordination()
    const cwd = this.sessions.get(sessionId)?.cwd || this.sessionMeta?.[sessionId]?.cwd || ''
    return (await this.workspaceSessions(sessionId)).slice(0, 15).map((item) => ({
      id: item.id,
      title: item.name || '',
      running: Boolean(item.streaming),
      modified: item.modified,
      recentFiles: coordination.recentFiles(item.id, cwd),
    }))
  }

  // 按 id、至少 6 位的 id 前缀或完整标题找同一文件夹里的会话。
  async resolveWorkspaceSession(sessionId, reference) {
    const ref = String(reference || '').trim()
    if (!ref) throw new Error('Name the session to message (its title or id from list_sessions).')
    const candidates = await this.workspaceSessions(sessionId)
    const lower = ref.toLowerCase()
    const byId = candidates.filter(
      (item) => item.id === ref || (ref.length >= 6 && item.id.startsWith(ref)),
    )
    const matches = byId.length
      ? byId
      : candidates.filter(
          (item) =>
            String(item.name || '')
              .trim()
              .toLowerCase() === lower,
        )
    if (matches.length === 1) return matches[0]
    const listed = candidates
      .slice(0, 10)
      .map((item) => `"${item.name || 'Untitled'}" (id ${item.id.slice(0, 8)})`)
      .join(', ')
    if (matches.length > 1)
      throw new Error(`More than one session matches "${ref}"; use the id instead: ${listed}.`)
    throw new Error(
      candidates.length
        ? `No session in this folder matches "${ref}". Sessions here: ${listed}.`
        : 'No other session works in this folder.',
    )
  }

  // session_message：对方在跑就插进它的下一步；没在跑先存进它的留言箱，不替它开一轮。
  async sendSessionMessage(sessionId, { session, text } = {}) {
    const body = String(text || '').trim()
    if (!body) throw new Error('The message is empty.')
    const coordination = this.getWorkspaceCoordination()
    const target = await this.resolveWorkspaceSession(sessionId, session)
    if (!coordination.allowSend(sessionId))
      throw new Error(
        `Not sent: a session can send at most ${SESSION_MESSAGE_LIMIT} messages an hour.`,
      )
    const message = coordination.newMessage({
      from: { id: sessionId, title: this.coordinationSession(sessionId)?.title || '' },
      text: body,
    })
    const value = this.sessions.get(target.id)
    // 引擎这一轮已经说完、只剩收尾时 steer 送不到任何一轮：当作没在跑。
    if (
      value?.session &&
      this.sessionRunIsActive(target.id, value) &&
      value.session.isStreaming !== false
    ) {
      try {
        await value.session.steer(sessionMessageContent([message], ATTACHMENT_MARKER))
        return { session: { id: target.id, title: target.name || '' }, delivered: 'now' }
      } catch {
        console.warn(
          'Could not deliver the session message into the running turn; saved to its inbox.',
        )
      }
    }
    await coordination.queueMessage(target.id, message)
    return { session: { id: target.id, title: target.name || '' }, delivered: 'later' }
  }

  // 会话页的“现在处理”：把留言箱里的留言作为一条消息交给这个会话，在后台开一轮。
  async deliverSessionMessages(id) {
    const sessionId = String(id || '').trim()
    if (!sessionId) throw pisperError('invalid', 'session')
    const coordination = this.getWorkspaceCoordination()
    const value = this.sessions.get(sessionId)
    if (this.sessionRunIsActive(sessionId, value) || this.startingRuns.has(sessionId))
      throw pisperError('busy', 'session', 'messages are delivered at its next step')
    const messages = await coordination.takeMessages(sessionId)
    if (!messages.length) return { started: false }
    void this.streamPrompt({
      sessionId,
      message: sessionMessageContent(messages, ATTACHMENT_MARKER),
      send: () => {},
    }).catch(async () => {
      // 没能开跑（模型不可用等）：留言放回去，会话页照旧显示。
      console.warn('Could not start a turn for session messages; messages were put back.')
      for (const message of messages)
        await coordination.queueMessage(sessionId, message).catch(() => {})
    })
    return { started: true }
  }

  async dismissSessionMessages(id) {
    const removed = await this.getWorkspaceCoordination().takeMessages(String(id || '').trim())
    return { removed: removed.length }
  }

  getTurnCheckpoints() {
    this.turnCheckpoints ||= new TurnCheckpointService({
      dataDir: this.dataDir,
      warn: () =>
        console.warn(
          'Could not write a file rewind snapshot; that file cannot be rewound by turn.',
        ),
    })
    return this.turnCheckpoints
  }

  // 回退文件的目标时间：某条消息（通常是一轮结束时的回复）的时间。会话运行中不允许。
  async fileRewindTarget(id, entryId) {
    const sessionId = String(id || '').trim()
    const targetId = String(entryId || '').trim()
    if (!sessionId || !targetId) throw pisperError('invalid', 'message', 'tree entry')
    if (this.sessionRunIsActive(sessionId, this.sessions.get(sessionId)))
      throw pisperError('busy', 'session')
    const value = await this.getOrCreateSession(sessionId)
    if (this.sessionRunIsActive(sessionId, value)) throw pisperError('busy', 'session')
    const entry = value.session.sessionManager.getEntry(targetId)
    if (entry?.type !== 'message') throw pisperError('not_found', 'message', 'tree entry')
    const at = Number.isFinite(entry.message?.timestamp)
      ? entry.message.timestamp
      : Date.parse(entry.timestamp)
    if (!Number.isFinite(at))
      throw pisperError('unsupported', 'message', 'no timestamp to rewind files to')
    return { sessionId, at }
  }

  async previewFileRewind(id, entryId) {
    const { sessionId, at } = await this.fileRewindTarget(id, entryId)
    return this.getTurnCheckpoints().preview(sessionId, at)
  }

  async rewindFiles(id, entryId) {
    const { sessionId, at } = await this.fileRewindTarget(id, entryId)
    const result = await this.getTurnCheckpoints().restore(sessionId, at)
    // 工作区文件变了：变更视图与看板摘要重新计算。
    this.streamProjection.invalidate(sessionId)
    this.boardEvents?.touch(sessionId)
    return result
  }

  // 流式执行一次会话提示：登记准备阶段 → 拿到会话运行时 → 校验未在运行 → 执行并清理中止标记。
  async streamPrompt(options) {
    return withRunStartTiming(() => this.streamPromptTimed(options))
  }

  async streamPromptTimed(options) {
    // 受理即算运行中：装配运行时、选工具等准备工作可能要几秒（新会话首轮、MCP 连接），这段时间里
    // 会话列表、看板、/messages 与 /live 都要报告“在跑”，否则别处打开会话会读到“空页、未运行”。
    // 同一会话的第二次 prompt 在准备阶段就拒绝，不再两轮并发装配。
    const requestedId = typeof options.sessionId === 'string' ? options.sessionId : ''
    if (requestedId && this.startingRuns.has(requestedId)) throw pisperError('busy', 'session')
    const message = typeof options.message === 'string' ? options.message : ''
    const starting = requestedId
      ? {
          aborted: false,
          // 供 /live 在准备阶段投影这条还没写进会话的消息；内部唤醒消息对用户隐藏，不投影。
          message: isInternalParentMessage(message)
            ? ''
            : message.split(ATTACHMENT_MARKER)[0].slice(0, MAX_PREPARING_MESSAGE_CHARS),
          acceptedAt: new Date().toISOString(),
        }
      : null
    if (starting) {
      this.startingRuns.set(requestedId, starting)
      this.streamProjection.invalidate(requestedId)
    }
    try {
      return await this.runStartedPrompt(options)
    } finally {
      // 正常路径在建立 live 时已移除；准备阶段失败或被拒绝时在这里收回，并让读者看到“已结束”。
      if (starting && this.startingRuns.get(requestedId) === starting) {
        this.startingRuns.delete(requestedId)
        this.streamProjection.invalidate(requestedId)
      }
      finishRunStartTiming('ended')
      if (starting?.aborted) void this.discardStoppedEmptySession(requestedId)
    }
  }

  // 第一条消息在准备阶段就被停止的新会话一条消息也没有：删掉，不在侧栏留下点进去是空白的会话。
  // 已有历史或还有排队输入的会话不动；删除失败只记一笔，不影响停止本身。
  async discardStoppedEmptySession(id) {
    try {
      const value = this.sessions.get(id)
      const messages = value ? value.session?.messages || [] : null
      const queued = value ? sessionInputQueueSnapshot(value.session).inputs.length : 0
      const empty = value
        ? messages.length === 0 && queued === 0 && !this.sessionRunIsActive(id, value)
        : this.pendingSessions.has(id) && !this.sessionRunIsActive(id)
      if (empty) await this.deleteSession(id)
    } catch {
      console.warn('Could not remove the empty session after stop.')
    }
  }

  async runStartedPrompt(options) {
    const finishSideChatRun = await this.sideChats?.beginRun(options.sessionId)
    markRunStage('sideChat')
    try {
      let value = await this.getOrCreateSession(options.sessionId)
      markRunStage('session')
      let id = value.session.sessionId
      if (value.forceDisposed && !value.runActive) {
        this.sessionLifecycle.disposeSessionRuntime(id, value, { force: true })
        value = await this.getOrCreateSession(options.sessionId)
        id = value.session.sessionId
      }
      if (this.sessionLifecycle.sessionEngineRunIsActive(id, value))
        throw pisperError('busy', 'session')
      // 历史会话不允许自动切换模型：绑定模型不可用时拒绝执行并显式告知，
      // 而不是静默用默认模型消耗额度、并把回退结果写回会话绑定。
      if (value.blockedModel) {
        const ref = `${value.blockedModel.provider}/${value.blockedModel.modelId}`
        const reason =
          value.blockedModel.reason === 'missing-auth'
            ? 'provider has no credentials'
            : 'model missing or provider not configured'
        throw pisperError('unavailable', 'model', `${ref}: ${reason}; pick another model`)
      }
      // 中止标记只属于单次运行：留在常驻运行时上会让下一次 prompt 继承上次的截止时间。
      delete value.abortedAt
      delete value.forceDisposed
      value.runActive = true
      try {
        this.installWorkspaceAssetCapture(value.session, value.cwd)
        markRunStage('assetCapture')
        return await this.runSessionPrompt(value, options)
      } finally {
        value.runActive = false
        // 运行收尾的两次落盘发生在 done/error 帧之后：落盘慢于看板 flush 时，最后一帧
        // 仍读到 runActive=true。这里在真正放下运行标记后再失效一次活动投影，
        // 经 onInvalidate 让看板事件流补发一帧“已结束”，/live 缓存也随之失效。
        this.streamProjection.invalidate(id, { transcript: false, activity: true, usage: false })
        // Bot 的对话这一轮说完：运行标记放下之后再收尾（见 finishBoardCard）。
        if (value.botTurnDue) {
          value.botTurnDue = false
          if (!value.abortedAt) this.scheduleAfterBotTurn(id)
          else this.dropBotChatReplies(id, value)
        } else if (value.pendingChatReplies?.length) {
          // 这一轮被停下或出错，不走收尾：在它说话时插话的聊天不会有回复了。告诉对方没答上，并把名单清掉——
          // 留着的话，下一轮（比如你私下问它的一句）的回复会被发到那个群里。
          this.dropBotChatReplies(id, value)
        }
        // Bot 在这一轮跑的时候改了名：运行中改不了会话标题，放下运行标记后补上。
        if (this.pendingThreadNames?.has(id)) void this.renameBotThread(id)
        // 运行期间注入失败、放回队列的 Agent 完成通知：本轮结束后补发；用户主动停止时不自动再起一轮。
        if (!value.abortedAt && value.pendingAgentNotifications?.length)
          this.scheduleAgentWakeup(id)
        // 这一轮结束（完成、失败、停止都算）：启动排在它后面的会话（sessionQueue）。
        void this.sessionQueue
          ?.onRunFinished(id)
          .catch(() => console.warn('Could not start the queued session.'))
        const forceDisposed = value.forceDisposed
        delete value.abortedAt
        delete value.forceDisposed
        if (forceDisposed) {
          // 被强制 dispose 的 Agent 在其遗留 prompt 落定前可能仍报告 isStreaming，
          // 但这个常驻运行时已不可复用，直接释放。
          this.sessionLifecycle.disposeSessionRuntime(id, value, { force: true })
        } else {
          this.touchSessionRuntime(value)
          this.evictIdleSessionRuntimes(id)
        }
      }
    } finally {
      await finishSideChatRun?.()
    }
  }

  async getPlugins(sessionId = '') {
    const state = await this.toolPlugins.getState()
    const stateTools = Array.isArray(state.tools) ? state.tools : []
    const statePlugins = Array.isArray(state.plugins) ? state.plugins : []
    const configuredToolNames = Array.isArray(state.enabledTools) ? state.enabledTools : []
    const supportedToolNames = new Set(
      Array.isArray(this.capabilities?.tools)
        ? this.capabilities.tools
        : stateTools.length
          ? stateTools.map((tool) => tool.id)
          : configuredToolNames,
    )
    const customPluginsAvailable = this.capabilities?.features?.plugins !== false
    const piExtensionsAvailable = this.capabilities?.features?.extensions !== false
    let extensionDashboard = { packages: [] }
    if (piExtensionsAvailable) {
      try {
        extensionDashboard = await this.skills.extensionDashboard({
          cwd: await this.sessionWorkspaceCwd(sessionId),
        })
      } catch {}
    }
    const piPlugins = [
      ...bridgePiExtensionPackages(extensionDashboard.packages).map((plugin) => ({
        ...plugin,
        enabled: state.piExtensions?.[plugin.packageSource] !== false,
        capabilities: plugin.capabilities.map((capability) => ({
          ...capability,
          enabled: state.piExtensions?.[plugin.packageSource] !== false,
        })),
      })),
    ]
    const plugins = [
      ...statePlugins
        .filter((plugin) => plugin.builtIn || customPluginsAvailable)
        .map((plugin) => ({
          ...plugin,
          capabilities: plugin.capabilities.filter(
            (capability) => !plugin.builtIn || supportedToolNames.has(capability.name),
          ),
        })),
      ...piPlugins,
    ].filter((plugin) => plugin.capabilities.length > 0)
    const projectedToolNames = plugins
      .filter((plugin) => !isPiExtensionPlugin(plugin))
      .flatMap((plugin) => plugin.capabilities.map((capability) => capability.name))
    const visibleToolNames = new Set(
      projectedToolNames.length
        ? projectedToolNames
        : configuredToolNames.filter((tool) => supportedToolNames.has(tool)),
    )
    const visibleState = {
      ...state,
      plugins,
      tools: stateTools.filter((tool) => visibleToolNames.has(tool.id)),
      presets: Object.fromEntries(
        Object.entries(state.presets || {}).map(([preset, tools]) => [
          preset,
          tools.filter((tool) => visibleToolNames.has(tool)),
        ]),
      ),
      enabledTools: state.enabledTools.filter((tool) => visibleToolNames.has(tool)),
    }
    if (!sessionId) return visibleState
    const enabledToolNames = this.toolPlugins
      .enabledTools({ enabledTools: visibleState.enabledTools })
      .filter((tool) => visibleToolNames.has(tool))
    const callablePiToolNames = piPlugins
      .filter((plugin) => plugin.name === 'Computer Use' && plugin.enabled !== false)
      .flatMap((plugin) => plugin.capabilities.map((capability) => capability.name))
    return {
      ...visibleState,
      // 可调用工具不随执行模式变化：模式只决定调用时是否逐次确认。
      callableToolNames: [...new Set([...enabledToolNames, ...callablePiToolNames])],
    }
  }

  // 默认工具集合初始化：为记忆/MCP 管理/Web 搜索等能力登记默认工具开关。
  async initializeToolPlugins() {
    await this.toolPlugins.init()
    await this.toolPlugins.ensureDefaultTools(['memory_search', 'memory_remember'], 'memoryToolsV1')
    await this.toolPlugins.ensureDefaultTools(
      ['memory_open', 'memory_confirm', 'memory_forget'],
      'memoryToolsV2',
    )
    await this.toolPlugins.ensureDefaultTools(['mcp_list', 'mcp_manage'], 'mcpManagementToolsV1')
    // 定时任务工具：既有安装若已物化 enabledTools，也补上（代理此前没有办法建定时任务）。
    await this.toolPlugins.ensureDefaultTools(
      ['schedule_list', 'schedule_manage'],
      'scheduleToolsV1',
    )
    await this.toolPlugins.ensureDefaultTools(['workflow_list', 'workflow_run'], 'workflowToolsV1')
    await this.toolPlugins.ensureDefaultTools(['web_search'], 'webSearchToolV1')
    await this.toolPlugins.ensureDefaultTools(['browser_automation'], 'browserAutomationToolV1')
    // 关过旧版 Computer Use 的人，新工具也保持关着。
    await this.toolPlugins.ensureDefaultTools(['computer_use'], 'computerUseToolV1', {
      skip: (config) => config.computerUseEnabled === false,
    })
    await this.toolPlugins.ensureDefaultTools(['skill_create'], 'skillCreateToolV1')
    await this.toolPlugins.ensureDefaultTools(['plugin_create'], 'pluginCreateToolV1')
    // 原生组件写入工具（A7）：既有安装若已物化 enabledTools，也补上它。
    await this.toolPlugins.ensureDefaultTools(['component_create'], 'componentCreateToolV1')
    await this.toolPlugins.ensureDefaultTools(['mobile_device'], 'mobileDeviceToolV2')
    // 生图默认开放：既有安装若已物化 enabledTools（如旧版 workspace 预设），需迁移补齐。
    await this.toolPlugins.ensureDefaultTools(['generate_visual'], 'visualGenerateToolV1')
  }

  getToolRisk(name) {
    return this.toolPlugins.getToolRisk(name) || this.mcp.getToolRisk(name)
  }

  testWebSearch(input) {
    return this.webSearch.test(input)
  }

  // 渠道（飞书/微信）发起的会话提示：不存在则建会话，支持隔离上下文与指定模型。
  async promptFromChannel({
    sessionId,
    message,
    attachments = [],
    cwd,
    title,
    model,
    executionMode,
    goalMode = false,
    teamMode = false,
    isolatedContext = false,
    requestedToolNames,
    allowedToolNames = null,
    // 执行节点（B6）：{ environmentId, cwd } 或 null；undefined 表示不改。
    executor,
    unattended = false,
    onSession,
    onEvent,
  }) {
    let id = String(sessionId || '')
    if (
      id &&
      !this.sessions.has(id) &&
      !this.pendingSessions.has(id) &&
      !(await this.findSessionInfo(id))
    )
      id = ''
    if (!id) {
      const created = await this.createSession(title || 'Feishu chat')
      id = created.id
      if (executor !== undefined) await this.setSessionExecutor(id, executor)
      if (cwd) await this.setSessionCwd(id, cwd)
    }
    if (isolatedContext) {
      const active = await this.getOrCreateSession(id)
      active.isolatedContext = true
      active.blockedToolNames = ISOLATED_CONTEXT_BLOCKED_TOOLS
    }
    if (executionMode) await this.setSessionExecutionMode(id, executionMode)
    if (model?.provider && model?.model) {
      const active = await this.getOrCreateSession(id)
      if (
        active.session.model?.provider !== model.provider ||
        active.session.model?.id !== model.model
      )
        await this.setSessionModel(id, model.provider, model.model)
    }
    onSession?.(id)
    let actualId = id
    let text = ''
    // 运行出错时 streamPrompt 只广播 error 事件、不抛出：记下来交给调用方，
    // 否则 Bot 任务会把失败的一轮当成“完成、没有改动”。
    let runError = ''
    // 最后一次工具调用之后说的话：Bot 拿它当回复，不把中间每一步的旁白拼进去。
    let finalText = ''
    const assetIds = new Set()
    await this.streamPrompt({
      sessionId: id,
      message,
      attachments,
      goalMode,
      teamMode,
      isolatedContext,
      requestedToolNames,
      allowedToolNames,
      unattended,
      send: (event, data) => {
        if ((event === 'meta' || event === 'done') && data?.sessionId) actualId = data.sessionId
        if (event === 'text_delta') {
          text += data?.delta || ''
          finalText += data?.delta || ''
        }
        if (event === 'tool_start') finalText = ''
        if (event === 'generated_asset' && data?.id) assetIds.add(data.id)
        if (event === 'error') runError = String(data?.message || 'Run failed.').slice(0, 500)
        onEvent?.(event, data)
      },
    })
    if (!text.trim()) {
      const messages = await this.getSessionMessages(actualId)
      text = [...messages].reverse().find((item) => item.role === 'agent')?.text || ''
    }
    const runtime = this.sessions.get(actualId)
    const assets = [...assetIds]
      .map((assetId) => this.assetIndex.assets.find((asset) => asset.id === assetId))
      .filter(Boolean)
      .map((asset) => ({
        id: asset.id,
        name: asset.name,
        path: asset.storagePath,
        mimeType: asset.mimeType,
      }))
      .filter((asset) => asset.path)
    return {
      sessionId: actualId,
      text: text.trim(),
      finalText: finalText.trim(),
      cwd: runtime?.cwd || this.sessionMeta[actualId]?.cwd || this.cwd,
      model: runtime?.session.model
        ? `${runtime.session.model.provider}/${runtime.session.model.id}`
        : '',
      assets,
      error: runError,
    }
  }

  async getChannels() {
    const state = this.channels.getState()
    const config = await this.getConfig()
    return {
      providers: state.providers,
      connections: state.connections,
      scopes: state.scopes,
      models: config.providers
        .filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured)
        .flatMap((provider) =>
          provider.models
            .filter((model) => model.kind === 'chat')
            .map((model) => ({
              provider: provider.id,
              model: model.id,
              label: `${provider.name} / ${model.name}`,
            })),
        ),
    }
  }

  startChannelOnboarding(platform, input = undefined) {
    return this.channels.startOnboarding(platform, input)
  }

  getChannelOnboarding(platform, id) {
    return this.channels.getOnboarding(platform, id)
  }

  cancelChannelOnboarding(platform, id) {
    return this.channels.cancelOnboarding(platform, id)
  }

  verifyChannelOnboarding(platform, id, code) {
    return this.channels.verifyOnboarding(platform, id, code)
  }

  async updateChannel(platform, input) {
    await this.channels.update(platform, input)
    return this.getChannels()
  }

  async reconnectChannel(platform) {
    await this.channels.connect(platform)
    return this.getChannels()
  }

  deleteChannel(platform) {
    return this.channels.remove(platform)
  }

  resetChannelScope(key) {
    return this.channels.resetScope(key)
  }

  getCompactionPreference() {
    return {
      thresholdPercent: this.compactionThresholdPercent,
      minPercent: MIN_COMPACTION_THRESHOLD_PERCENT,
      maxPercent: MAX_COMPACTION_THRESHOLD_PERCENT,
    }
  }

  getChatDockLayout() {
    return readJson(this.chatDockLayoutPath, null)
  }

  async saveChatDockLayout(input) {
    if (!input || typeof input !== 'object' || Array.isArray(input)) {
      throw pisperError('invalid', 'config', 'dock layout must be a JSON object')
    }
    const snapshot = structuredClone(input)
    this.chatDockLayoutWrite = this.chatDockLayoutWrite
      .catch(() => {})
      .then(() => writeJsonAtomic(this.chatDockLayoutPath, snapshot))
    await this.chatDockLayoutWrite
    return snapshot
  }

  // 自动确认阈值已取消（记忆不需要确认）。旧版前端还会读写这个设置，给固定值、写入不生效。
  getMemoryPreference() {
    return { autoApproveConfidence: 0, minConfidence: 0, maxConfidence: 100 }
  }

  async updateMemoryPreference() {
    return this.getMemoryPreference()
  }

  getBetaFeatures() {
    return {
      features: projectBetaFeatures(
        this.betaFeatureFlags,
        this.capabilities?.features,
        this.betaFeatureCatalog,
      ),
    }
  }

  // 服务在做决定的那一刻调用（例如渠道消息是否交给 Bot），不缓存，改开关立即生效。
  isBetaFeatureEnabled(id) {
    return isBetaFeatureActive(
      this.betaFeatureFlags,
      this.capabilities?.features,
      id,
      this.betaFeatureCatalog,
    )
  }

  // betaFeatureCatalog 默认为空（undefined → 用 shared 里的正式清单），只有测试会注入。
  // 写入串行化：连续切换两个开关时，后一次必须基于前一次写入后的配置，否则会丢更新。
  // 先校验再排队，非法输入不占写入队列；配置里清单之外的旧键原样保留。
  async updateBetaFeatures(input) {
    applyBetaFlagPatch(this.betaFeatureFlags, input, this.betaFeatureCatalog)
    const write = this.betaFeatureWrite
      .catch(() => {})
      .then(async () => {
        const appConfig = await readJson(this.appConfigPath, {})
        const flags = applyBetaFlagPatch(
          normalizeStoredBetaFlags(appConfig.betaFeatures),
          input,
          this.betaFeatureCatalog,
        )
        await writeJsonAtomic(this.appConfigPath, { ...appConfig, betaFeatures: flags })
        this.betaFeatureFlags = flags
      })
    this.betaFeatureWrite = write
    await write
    // Bot 的会话只在 Bots 打开时出现在列表与看板上：开关一变就让在线的看板重算这些会话
    // （关掉的发 removed，打开的发卡片），并通知目录重读，界面不用刷新。
    if (this.bots) {
      for (const sessionId of this.botSessionProjects().keys()) this.boardEvents?.touch(sessionId)
      this.boardEvents?.catalogChanged()
    }
    return this.getBetaFeatures()
  }

  // 列表里带上每个 Bot 此刻的动态（在跑、在等批准或回答），侧栏据此显示头像状态与“需要你”。
  listBots() {
    return this.bots.list().map((bot) => ({ ...bot, activity: this.botActivity(bot) }))
  }

  getBot(id) {
    return this.bots.get(id)
  }

  // Bot 就是它的长期会话加上设定：动态直接读会话。审批与提问只在运行中出现。
  // Bot 此刻的动态：对话在说话，或者有子 agent 在干活，都算在忙；在等批准或回答时需要你。
  // working = 在做的子 agent 件数，current = 正在做的那件（头像悬停时显示）。
  botActivity(bot) {
    const sessionId = bot?.threadSessionId
    if (!sessionId) return null
    const talking = this.sessionRunIsActive(sessionId) || this.startingRuns.has(sessionId)
    // 后台命令也算手上的活。
    // Bots 列表每几秒轮询一次：手上没有在跑的活时，不为每个 Bot 把子 agent 记录过滤、排序、投影一遍。
    const mayWork =
      !this.multiAgents?.hasActive ||
      this.multiAgents.hasActive(sessionId) ||
      Boolean(this.botCommands?.list(sessionId)?.length)
    const agents = mayWork
      ? this.listSessionAgents(sessionId).filter((agent) =>
          ['queued', 'starting', 'running'].includes(agent.status),
        )
      : []
    const { approval, question } = this.botWaiting(sessionId)
    const waiting = approval ? 'approval' : question ? 'question' : null
    if (!talking && !agents.length && !waiting)
      return { status: 'idle', waiting: null, needsYou: false, working: 0, current: '' }
    const shown = agents.find((agent) => agent.status === 'running') || agents[0]
    const current = shown ? shown.title || shown.taskName : ''
    return {
      status: talking || agents.length ? 'running' : 'idle',
      waiting,
      needsYou: Boolean(waiting),
      working: agents.length,
      current,
    }
  }

  // 会话派出的子 agent 摘要与 Bot 对话转到后台的命令（时间线事件用，按开始时间排）。
  listSessionAgents(sessionId) {
    const id = String(sessionId || '')
    const agents = this.multiAgents?.summaries(id) || []
    const commands = this.botCommands?.list(id) || []
    if (!commands.length) return agents
    return [...agents, ...commands].sort((left, right) =>
      String(left.startedAt).localeCompare(String(right.startedAt)),
    )
  }

  // 只读查看会话自己的某个子 agent 的过程；后台命令没有对话，和不是它的子 agent 一样返回 null。
  getSessionAgentTranscript(sessionId, agentId) {
    if (!this.multiAgents || String(agentId).startsWith('cmd_')) return null
    return projectAgentTranscript(
      this.multiAgents.transcriptSource(String(sessionId || ''), String(agentId || '')),
    )
  }

  // 停掉会话自己的某个子 agent 或后台命令；不是它的、或已经结束的返回 null。
  stopSessionAgent(sessionId, agentId) {
    if (String(agentId).startsWith('cmd_'))
      return this.botCommands?.stop(String(sessionId || ''), agentId) || null
    const agent = this.multiAgents
      ?.summaries(String(sessionId || ''))
      .find((item) => item.id === agentId)
    if (!agent || !['queued', 'starting', 'running'].includes(agent.status)) return null
    return this.multiAgents.interrupt(sessionId, agentId, "Stopped at the user's request.")
  }

  // 会话属于哪个 Bot（Bots 实验功能关着时一律不算）：运行时据此套用 Bot 的指令、工具范围与隔离上下文。
  botForSession(sessionId) {
    if (!sessionId || !this.bots || !this.capabilities.features.bots) return null
    if (!this.isBetaFeatureEnabled('bots')) return null
    // 每次权限判断都会问到这里：按会话直接找，不把全部 Bot 拷贝一遍。
    if (this.bots.forSession) return this.bots.forSession(sessionId)
    return this.bots.list().find((bot) => bot.threadSessionId === sessionId) || null
  }

  // 新建时可以只给名字：没选目录就在本机的默认工作目录里干活，之后在设置里再换。
  // 建好就开它的长期会话，打开 Bot 就是打开这个会话。
  async createBot(input) {
    const value = input && typeof input === 'object' ? input : {}
    const workspace = value.workspace && typeof value.workspace === 'object' ? value.workspace : {}
    const environmentId = String(workspace.environmentId || 'local')
    const cwd = String(workspace.cwd ?? '').trim()
    const bot = await this.bots.create(
      cwd || environmentId !== 'local'
        ? value
        : { ...value, workspace: { ...workspace, cwd: this.cwd } },
    )
    await this.ensureBotSession(bot.id)
    return this.bots.get(bot.id)
  }

  // 设定变了就套到会话上：目录、电脑、权限、模型立即生效；指令与工具范围在会话空闲时重建运行时后生效
  // （正在跑的这一轮不打断，下一轮起用新设定）。
  async updateBot(id, input) {
    const before = this.bots.get(id)
    const bot = await this.bots.update(id, input)
    if (!bot?.threadSessionId) return bot
    await this.applyBotSettings(bot, bot.threadSessionId)
    // 会话标题就是 Bot 的名字：改名时跟着改，侧栏、看板与会话页头一致。
    if (before && before.name !== bot.name) {
      ;(this.pendingThreadNames ??= new Map()).set(bot.threadSessionId, bot.name)
      await this.renameBotThread(bot.threadSessionId)
    }
    // 关掉主动巡看：正在跑的巡看也停下，之后不再巡看。
    if (before?.proactive?.enabled && !bot.proactive?.enabled) {
      for (const agent of this.multiAgents?.list(bot.threadSessionId) || []) {
        if (
          this.botProactiveAgents?.has(agent.id) &&
          ['queued', 'starting', 'running'].includes(agent.status)
        )
          this.multiAgents.interrupt(
            bot.threadSessionId,
            agent.id,
            'Proactive checks were turned off.',
          )
      }
    }
    return bot
  }

  // 把 Bot 会话的标题改成记下的新名字。会话正在跑时改不了（会话服务拒绝改标题），
  // 名字留在 pendingThreadNames，等这一轮结束（runStartedPrompt 收尾）再改，不能悄悄丢掉。
  async renameBotThread(sessionId) {
    const name = this.pendingThreadNames?.get(sessionId)
    if (!name) return
    try {
      await this.renameSession(sessionId, name)
      if (this.pendingThreadNames.get(sessionId) === name) this.pendingThreadNames.delete(sessionId)
    } catch (error) {
      if (error?.code !== 'busy') this.pendingThreadNames.delete(sessionId)
    }
  }

  // 打开 Bot：返回它的长期会话，还没有（或会话文件已经不在）就新建一个。
  async ensureBotSession(botId) {
    const bot = this.bots.get(botId)
    if (!bot)
      throw Object.assign(new Error('Bot not found.'), { code: 'bot_not_found', statusCode: 404 })
    const existing = bot.threadSessionId
    if (
      existing &&
      (this.sessions.has(existing) ||
        this.pendingSessions.has(existing) ||
        (await this.findSessionInfo(existing)))
    )
      return existing
    const created = await this.createSession(bot.name, await this.botSessionCwd(bot))
    await this.bots.setThread(bot.id, created.id)
    await this.applyBotSettings(bot, created.id)
    return created.id
  }

  // Bot 的电脑是执行节点（B6）时，工具在那台电脑的 cwd 上执行，大脑这边的会话目录只是占位。
  async botSessionCwd(bot) {
    const environmentId = bot.workspace.environmentId || 'local'
    return environmentId === 'local'
      ? bot.workspace.cwd
      : this.executorSessionDirectory(environmentId)
  }

  async applyBotSettings(bot, sessionId) {
    const environmentId = bot.workspace.environmentId || 'local'
    await this.setSessionExecutor(
      sessionId,
      environmentId === 'local' ? null : { environmentId, cwd: bot.workspace.cwd },
    )
    // 运行中换不了目录（会话服务拒绝）：这次先跳过，其余设定照常生效；跑完后再保存一次设定即可。
    await this.setSessionCwd(sessionId, await this.botSessionCwd(bot)).catch(() => {})
    await this.setSessionExecutionMode(sessionId, bot.executionMode)
    const resolved = await this.providerPreferences.resolveSubagentModel(
      bot.model.ref ? { modelRef: bot.model.ref } : { tier: bot.model.tier },
    )
    if (resolved.model)
      await this.setSessionModel(sessionId, resolved.model.provider, resolved.model.id).catch(
        () => {},
      )
    // 思考等级：没设就用模型默认；模型不支持这一档时会话服务会拒绝，按模型默认处理。
    if (bot.thinking) await this.setSessionThinkingLevel(sessionId, bot.thinking).catch(() => {})
    // 指令进系统提示、工具范围进运行时：空闲时释放运行时，下次用到时按新设定重建。
    // 受保护时不释放：正在说话、还有子 agent 在跑（批准挂在这个会话上）、或者刚做完的活的完成通知还在
    // 等唤醒（释放会把它清掉，对话就收不到汇报）；下次空闲时再按新设定重建。
    const value = this.sessions.get(sessionId)
    if (!this.sessionRuntimeIsProtected(sessionId, value))
      this.disposeSessionRuntime(sessionId, value)
  }

  // 渠道、定时任务对 Bot 说一句话：发进它的长期会话。这一轮结束后把回复发回来源聊天。
  // Bot 正在说话时：定时任务排在这一轮之后（不需要回复）；聊天里的人收到“在忙”，不排队等很久才回。
  // waitForStart：等到这一轮真的开始（或开不起来）再返回；开不起来时抛出原因。定时任务用它，
  // 不然模型不可用、撞上正忙时任务记录仍写着“已发送”。
  async messageBot(botId, { text, source = { kind: 'chat' }, waitForStart = false } = {}) {
    const bot = this.bots.get(botId)
    if (!bot)
      throw Object.assign(new Error('Bot not found.'), { code: 'bot_not_found', statusCode: 404 })
    if (!bot.enabled)
      throw Object.assign(new Error(`${bot.name} is paused.`), {
        code: 'bot_paused',
        statusCode: 409,
      })
    const message = normalizeBotMessage(text)
    const sessionId = await this.ensureBotSession(botId)
    // 这句话是谁说的，决定这一轮 bot_remember 写入的可信度：外部的话不能冒充用户的话。
    const trust = botSourceTrust(source)
    if (this.sessionRunIsActive(sessionId) || this.startingRuns.has(sessionId)) {
      // 对话正在说话：定时的话排在这一轮之后；别的话（聊天、别的 Bot、问题的回答）插进来马上处理，不阻塞。
      // 聊天来的话，这一轮说完后把回复发回那个聊天（afterBotTurn）。
      const value = this.sessions.get(sessionId)
      const interrupt = source.kind !== 'schedule' && Boolean(value?.conversationOnly)
      if (!interrupt && source.kind !== 'schedule' && source.kind !== 'bot')
        return { sessionId, status: 'busy' }
      const queued = await this.queueSessionMessage(sessionId, {
        message,
        behavior: interrupt ? 'interrupt' : 'followUp',
      }).catch(() => null)
      if (queued) {
        if (source.kind === 'channel') (value.pendingChatReplies ||= []).push(source)
        if (value) value.botTurnTrust = stricterBotTrust(value.botTurnTrust, trust)
        return { sessionId, status: 'queued' }
      }
      // 正好在这一刻说完了：当成新的一句发进去。
    }
    if (source.kind === 'channel') this.botChatOrigins.set(sessionId, source)
    // 运行开始时取走（runSessionPrompt）。还有一份没取走时按更不可信的那个算：会话还没加载时开跑前要等一会儿，
    // 这段空档里第二个来源直接覆盖，会让外部的话那一轮按“用户说的”写记忆。
    const waitingTrust = this.botPendingTrust?.get(sessionId)
    this.botPendingTrust?.set(
      sessionId,
      waitingTrust ? stricterBotTrust(waitingTrust, trust) : trust,
    )
    let settleStart = () => {}
    const started = new Promise((resolveStart) => {
      settleStart = resolveStart
    })
    void this.promptFromChannel({
      sessionId,
      message,
      unattended: source.kind === 'schedule' || source.kind === 'bot',
      // meta 是运行状态建立后的第一帧：到这里这一轮就算开始了。
      onEvent: (event) => {
        if (event === 'meta') settleStart('')
      },
    })
      .then((result) => {
        settleStart(result?.error || '')
        return this.notifyBotChat(source, botReplyText(bot.name, result))
      })
      .catch((error) => {
        settleStart(error instanceof Error ? error.message : String(error))
        this.botPendingTrust?.delete(sessionId)
        return this.notifyBotChat(
          source,
          botReplyText(bot.name, { error: error instanceof Error ? error.message : String(error) }),
        )
      })
      .finally(() => {
        if (this.botChatOrigins.get(sessionId) === source) this.botChatOrigins.delete(sessionId)
      })
    if (waitForStart) {
      const failure = await started
      if (failure) throw pisperError('failed', 'bot', failure)
    }
    return { sessionId, status: 'started' }
  }

  // 没说完的一轮（停下、出错）：告诉在等回复的聊天这次没答上，名单清空。
  // 是你自己点的停止：只清名单，不往外面的聊天里发“没做成”（停下是你的决定，不用替 Bot 向群里解释）。
  dropBotChatReplies(sessionId, value) {
    const sources = (value.pendingChatReplies || []).splice(0)
    if (value.abortedAt) return
    const bot = this.botForSession(sessionId)
    if (!bot) return
    for (const source of sources)
      this.notifyBotChat(source, botReplyText(bot.name, { error: 'the reply was interrupted' }))
  }

  // 一轮开始时定下这一轮的话是谁说的（见 botSourceTrust），bot_remember 按它定可信度。
  // 不是 messageBot 叫起来的：有人在场就是应用里的用户，没人在场（定时、子 agent 做完的唤醒）不能算用户说的。
  takeBotTurnTrust(sessionId, unattended = false) {
    const pending = this.botPendingTrust?.get(sessionId)
    this.botPendingTrust?.delete(sessionId)
    return pending || (unattended ? 'agent' : 'user')
  }

  // Bot 的对话每轮附带的状态快照：手上的子 agent 与后台命令、各自状态与最近进展、在等几个批准、
  // 可以挑的模型、可以找的别的 Bot、会的技能。记忆不在这里：相关的按这句话检索后单独注入（见 turnContext）。
  async botStateContext(sessionId) {
    const bot = this.botForSession(sessionId)
    const skills = bot ? await this.botSkills?.list(bot.id).catch(() => []) : []
    const models = bot
      ? await this.providerPreferences?.availableChatModelRefs?.().catch(() => [])
      : []
    const bots = bot
      ? (this.bots?.list?.() || [])
          .filter((item) => item.id !== bot.id && item.enabled)
          .map((item) => ({ name: item.name, instructions: item.instructions }))
      : []
    return [
      botStateSnapshot({
        agents: this.multiAgents?.list(sessionId) || [],
        commands: (this.botCommands?.list(sessionId) || []).map((command) => ({
          id: command.id,
          command: command.message,
          status: command.status,
        })),
        waitingApprovals: this.permissions?.getPending?.(sessionId)?.length || 0,
        limit: bot?.maxParallelAgents || BOT_MAX_PARALLEL_AGENTS,
        models: models || [],
        bots,
      }),
      botSkillsContext(skills || []),
    ]
      .filter(Boolean)
      .join('\n\n')
  }

  // 对话这一轮说完（运行标记放下之后）：收尾。同一个对话的收尾按顺序跑。
  scheduleAfterBotTurn(sessionId) {
    this.botTurnChains ||= new Map()
    const previous = this.botTurnChains.get(sessionId) || Promise.resolve()
    const next = previous
      .then(() => this.afterBotTurn(sessionId))
      .catch(() => console.warn('[bots] Could not finish the bot turn.'))
      .finally(() => {
        if (this.botTurnChains.get(sessionId) === next) this.botTurnChains.delete(sessionId)
      })
    this.botTurnChains.set(sessionId, next)
    return next
  }

  // 收尾：别的 Bot 在等回话的，把回复交回去；聊天在它说话时发来的话，把回复发回那个聊天；
  // 巡看结果用完就忘掉标记。记忆的整理归统一的后台整理，不在这里。
  async afterBotTurn(sessionId) {
    const value = this.sessions.get(sessionId)
    const bot = this.botForSession(sessionId)
    if (!value?.conversationOnly || !bot) return null
    const trigger = botTurnTrigger(value.session.messages || [])
    await this.forwardBotReplies(bot, value, trigger).catch(() =>
      console.warn('[bots] Could not pass the reply to the other bot.'),
    )
    for (const source of (value.pendingChatReplies || []).splice(0))
      this.notifyBotChat(source, botReplyText(bot.name, { text: trigger.reply }))
    for (const id of trigger.completedAgents) this.botProactiveAgents?.delete(id)
    return trigger
  }

  // 巡看结果与每日简报那一轮只出建议（用户说“做吧”的那一轮才照常）。
  botTurnIsQuiet(trigger) {
    return trigger.brief || trigger.completedAgents.some((id) => this.botProactiveAgents?.has(id))
  }

  // Bot 的对话里这个工具要批准时为什么不等，见 botConversationApprovalRefusal。
  botApprovalRefusal(sessionId, toolName) {
    const value = this.sessions.get(sessionId)
    return botConversationApprovalRefusal({
      toolName,
      unattended: Boolean(value?.unattendedRun),
      quiet: this.botTurnIsQuiet(botTurnTrigger(value?.session?.messages || [])),
    })
  }

  // bot_* 工具背后的运行时：每次调用时按这一刻的会话与 Bot 设定执行，只碰这个 Bot 自己的活、记忆与定时。
  // 只出建议的那一轮，派活、找别的 Bot 一律拒绝。
  botConversationRuntime(sessionId) {
    const context = () => {
      const value = this.sessions.get(sessionId)
      const bot = this.botForSession(sessionId)
      if (!value?.session || !bot)
        throw new Error('This is not a bot conversation; the tool is unavailable.')
      const trigger = botTurnTrigger(value.session.messages || [])
      return { value, bot, trigger, quiet: this.botTurnIsQuiet(trigger) }
    }
    const act = async (input, { work = false } = {}) => {
      const { value, bot, trigger, quiet } = context()
      const decision = normalizeBotAction(input)
      if (!decision) throw new Error('Arguments are incomplete or too long.')
      if (work && quiet)
        throw new Error(
          'This turn reports a read-only check or the daily brief: suggest, do not start work or message other bots. Wait for the user to say so.',
        )
      return this.executeBotDecision(sessionId, value, bot, decision, trigger)
    }
    return {
      startJob: async ({ title, task, model, tier }) => {
        const result = await act({ type: 'spawn', title, task, model, tier }, { work: true })
        return {
          ...result,
          text: `Started job ${result.id.slice(0, 8)} "${result.title}" in the background. You will be told when it finishes; do not wait for it.`,
        }
      },
      addToJob: async ({ job, text }) => {
        const result = await act({ type: 'add', agent: job, text })
        return {
          ...result,
          text: `Sent to job ${result.id.slice(0, 8)}; it gets it after its current step.`,
        }
      },
      stopJob: async ({ job }) => {
        const command = (this.botCommands?.list(sessionId) || []).find((item) =>
          item.id.startsWith(String(job)),
        )
        if (command) {
          this.botCommands.stop(sessionId, command.id)
          return { type: 'stop', id: command.id, text: `Stopped background command ${command.id}.` }
        }
        const result = await act({ type: 'stop', agent: job })
        return { ...result, text: `Stopped job ${result.id.slice(0, 8)}.` }
      },
      ask: async ({ questions }) => {
        const { bot } = context()
        const normalized = normalizeQuestions(questions)
        // 不等回答：卡片挂在对话上，回答作为下一条消息说进来（对话在说话时插进去）。
        void this.questions
          .request({ sessionId, toolCallId: `bot_ask_${randomUUID()}`, questions: normalized })
          .then((result) =>
            result?.outcome === 'cancelled'
              ? null
              : this.messageBot(bot.id, {
                  text: botAnswerText(normalized, result),
                  source: { kind: 'answer' },
                }),
          )
          .catch(() => console.warn('[bots] Could not return the answer to the bot conversation.'))
        return {
          type: 'ask',
          text: 'The question card is now shown to the user. End your turn here; their answer will arrive as the next message.',
        }
      },
      schedule: async (input) => {
        const { value, bot } = context()
        const own = () =>
          (this.schedules?.getState?.().tasks || []).filter(
            (task) => task.targetType === 'bot' && task.botId === bot.id,
          )
        if (input.action === 'list') {
          const tasks = own()
          return {
            type: 'schedule',
            text: tasks.length
              ? tasks
                  .map(
                    (task) =>
                      `${task.id} · ${task.frequency} ${task.date || ''} ${task.time || ''} · ${task.enabled ? 'on' : 'off'} · ${task.botMessage}`,
                  )
                  .join('\n')
              : 'No schedules yet.',
          }
        }
        if (input.action === 'remove') {
          const task = own().find((item) => item.id === String(input.id || ''))
          if (!task) throw new Error('No such schedule, or it is not yours.')
          await this.schedules.remove(task.id)
          return { type: 'schedule', text: `Removed schedule ${task.id}.` }
        }
        // 没人在场的运行（定时、别的 Bot 叫起来的）里不建新的定时，避免自己不停地给自己排活。
        if (value.unattendedRun)
          throw new Error('Schedules cannot be created in an unattended turn.')
        // 在群里定的：消息里写明是哪个群，到点那一轮才知道往哪个群里说（bot_notify to=group）。
        const origin = this.botChatOrigins?.get(sessionId)
        const where =
          origin?.chatType === 'group'
            ? ` (scheduled in a ${CHANNEL_NAMES[origin.platform] || origin.platform} group, group ID ${origin.peerId})`
            : ''
        const message = where
          ? `${String(input.message ?? '').slice(0, 4000 - where.length)}${where}`
          : input.message
        const created = await this.schedules.create(
          botScheduleInput({ bot, input: { ...input, message } }),
          {
            createdBy: { kind: 'agent', sessionId },
          },
        )
        return {
          type: 'schedule',
          id: created?.id,
          text: `Scheduled (${created?.id || 'ok'}): ${created?.frequency || ''} ${created?.date || ''} ${created?.time || ''} ${created?.timezone || ''}. At that time the message arrives here.`,
        }
      },
      remember: async ({ text, source, again }) => {
        const result = await act({ type: 'remember', text, source, again })
        return {
          ...result,
          text: result.skipped
            ? 'Not stored: the user deleted this memory earlier. If they have just asked you to remember it again, call bot_remember with again=true; otherwise leave it.'
            : 'Remembered.',
        }
      },
      forget: async ({ memory }) => {
        const result = await act({ type: 'forget', memory })
        return { ...result, text: 'Forgotten.' }
      },
      recall: async (input) => {
        const { bot } = context()
        const memories = await this.botMemory.search(bot, input)
        return { type: 'recall', count: memories.length, memories }
      },
      message: async ({ bot, text, expectsReply }) => {
        const result = await act({ type: 'message', bot, text, expectsReply }, { work: true })
        return {
          ...result,
          text: result.skipped
            ? `Not sent (${result.skipped}).`
            : 'Sent. Its reply will come back here automatically.',
        }
      },
      // 默认发到用户自己和机器人的私聊；to=group 时发到 Bot 规则里的群。开头带上 Bot 的名字，看的人知道是谁说的。
      notify: async ({ text, platform, to, group }) => {
        const { value, bot } = context()
        const message = String(text ?? '').trim()
        if (!message || message.length > 2_000)
          throw new Error('The notification needs text, at most 2000 characters.')
        const now = Date.now()
        const recent = (this.botNotifyTimes.get(bot.id) || []).filter(
          (time) => now - time < BOT_NOTIFY_WINDOW_MS,
        )
        if (recent.length >= BOT_NOTIFY_LIMIT)
          throw new Error(
            `Not sent: already ${BOT_NOTIFY_LIMIT} notifications in the last hour. Put it in your reply here instead.`,
          )
        if (to === 'group' || group) {
          // 群里其他人或别的 Bot 叫起来的那一轮不能往群里推：别人的话不能让它去别的群里说。
          if (value.botTurnTrust === 'external')
            throw new Error(
              'Not sent: this turn was started by someone other than the user, so it cannot post to groups.',
            )
          const target = this.botGroupTarget(bot, {
            group,
            platform,
            origin: this.botChatOrigins?.get(sessionId),
          })
          await this.channels.notifyGroup(
            target.platform,
            target.peerId,
            botNotifyText(bot.name, message),
          )
          this.botNotifyTimes.set(bot.id, [...recent, now])
          return {
            type: 'notify',
            sent: [target.platform],
            group: target.peerId,
            text: `Posted to the ${target.platform} group ${target.title ? `"${target.title}" ` : ''}(${target.peerId}).`,
          }
        }
        const results =
          (await this.channels?.notifyOwner(botNotifyText(bot.name, message), {
            platforms: platform ? [String(platform)] : undefined,
          })) || []
        const sent = results.filter((result) => result.status === 'sent')
        if (sent.length) this.botNotifyTimes.set(bot.id, [...recent, now])
        const reasons = results
          .filter((result) => result.status !== 'sent')
          .map((result) =>
            result.status === 'no_chat'
              ? `${result.platform}: the user has never messaged the Pisper bot in a private chat there, so it cannot write first`
              : `${result.platform}: ${result.error}`,
          )
        if (!sent.length)
          throw new Error(
            results.length
              ? `Not sent. ${reasons.join('; ')}.`
              : 'Not sent: no chat app is connected to Pisper (or not the one asked for). Tell the user to connect one under Channels.',
          )
        return {
          type: 'notify',
          sent: sent.map((result) => result.platform),
          text: `Sent to ${sent.map((result) => result.platform).join(', ')}.${reasons.length ? ` Not sent: ${reasons.join('; ')}.` : ''}`,
        }
      },
    }
  }

  async executeBotDecision(sessionId, value, bot, decision, trigger = null) {
    if (decision.type === 'message') return this.sendBotMessage(bot, decision, trigger)
    if (decision.type === 'remember') {
      const entry = await this.botMemory.remember(bot.id, decision.text, decision.source, {
        trust: value?.botTurnTrust,
        again: decision.again,
      })
      return entry.forgotten
        ? { type: 'remember', id: '', skipped: 'forgotten' }
        : { type: 'remember', id: entry.id }
    }
    if (decision.type === 'forget') {
      // 给完整 id 或它的前几位都行（不带 mem_ 也行）：只认唯一对上的那一条。
      const prefix = decision.memory.replace(/^mem_/, '')
      const entries = await this.botMemory.list(bot.id)
      const matches = entries.filter((entry) => entry.id.slice(4).startsWith(prefix))
      if (matches.length !== 1) throw new Error('The memory to forget does not match any memory.')
      await this.botMemory.forget(bot.id, { id: matches[0].id })
      return { type: 'forget', id: matches[0].id }
    }
    if (decision.type === 'spawn') {
      // 模型由 Bot 按活挑：选的模型用不了时退回档位，再退回 Bot 的默认模型。
      const chosen = decision.model
        ? await this.providerPreferences.resolveSubagentModel({ modelRef: decision.model })
        : null
      const instructions = await this.botAgentInstructions(bot, decision.task)
      const agent = await value.multiAgentRuntime.spawn({
        taskName: decision.title,
        title: decision.title,
        ...(instructions ? { instructions } : {}),
        message: decision.task,
        ...(decision.tier ? { tier: decision.tier } : {}),
        ...(chosen?.model ? { resolvedModel: chosen.model } : {}),
        // 工具按 Bot 的工具范围给（null = 当前启用的全部工具）；对话自己没有可用工具，不能按它继承。
        toolNames: Array.isArray(bot.tools) ? bot.tools : value.baseToolNames || [],
        parentLimit: bot.maxParallelAgents || BOT_MAX_PARALLEL_AGENTS,
        // 它自己写的技能：技能加载器列进子 agent 的系统提示，用到时再读全文。
        ...(this.botSkills ? { skillPaths: [this.botSkills.dir(bot.id)] } : {}),
      })
      // 别的 Bot 请它干的活：记下是谁请的，做完把汇报交回去；那边的“在等回话”继续等。
      const request = trigger?.message?.reply ? trigger.message : null
      if (request && this.botLinks) {
        this.botAgentRequests ||= new Map()
        this.botAgentRequests.set(agent.id, {
          fromBotId: request.from,
          chain: request.chain,
          hop: request.hop,
        })
        this.botLinks.advance(request.chain, { hop: request.hop, waiting: true })
      }
      return { type: 'spawn', id: agent.id, title: decision.title }
    }
    const agent = resolveBotAgent(this.multiAgents.list(sessionId), decision.agent)
    if (!agent) throw new Error('The job does not match any of your jobs.')
    if (decision.type === 'add') {
      await this.multiAgents.followup(sessionId, agent.id, decision.text)
      return { type: 'add', id: agent.id }
    }
    this.multiAgents.interrupt(sessionId, agent.id, "Stopped at the user's request.")
    return { type: 'stop', id: agent.id }
  }

  // 子 agent 带上 Bot 的工作说明与记忆：它不记事，背景从这里来。记忆是画像加上和这件活相关的那些。
  async botAgentInstructions(bot, task = '') {
    const memory = await this.botMemory.agentContext(bot, { task, cwd: bot.workspace?.cwd })
    return [
      String(bot.instructions || '').trim()
        ? `[${bot.name}'s instructions]\n${bot.instructions.trim()}`
        : '',
      memory,
    ]
      .filter(Boolean)
      .join('\n\n')
  }

  // C6 主动（默认关）：每分钟看一眼开了主动的 Bot，空闲且到点时做只读巡看或写每日简报。
  // 计时器由运行时持有（startBotProactiveTimer / dispose 清掉）；实验功能关着时什么都不做。
  async botProactiveTick(now = Date.now()) {
    if (!this.bots || !this.capabilities.features.bots || !this.isBetaFeatureEnabled('bots'))
      return []
    const started = []
    for (const bot of this.bots.list()) {
      const proactive = bot.proactive || {}
      if (!bot.enabled || (!proactive.enabled && !proactive.brief)) continue
      if ((bot.workspace.environmentId || 'local') !== 'local' || !bot.threadSessionId) continue
      const sessionId = bot.threadSessionId
      // 空闲：对话没在说话、手上没活、没有在等你的批准。
      if (this.sessionRunIsActive(sessionId) || this.startingRuns.has(sessionId)) continue
      if (this.multiAgents.hasActive(sessionId)) continue
      if (this.permissions?.getPending?.(sessionId)?.length) continue
      const state = await this.botProactive.read(bot.id).catch(() => null)
      if (!state) continue
      const due = botProactiveDue(bot, state, now)
      try {
        if (due.brief) {
          await this.botProactive.write(bot.id, { lastBriefDay: due.day })
          await this.sendBotBrief(bot, now)
          started.push({ bot: bot.id, kind: 'brief' })
        } else if (due.check) {
          await this.botProactive.write(bot.id, { lastCheckAt: new Date(now).toISOString() })
          const agent = await this.runBotProactiveCheck(bot)
          if (agent) started.push({ bot: bot.id, kind: 'check', agent: agent.id })
        }
      } catch {
        console.warn('[bots] Could not start the proactive check.')
      }
    }
    return started
  }

  // 只读巡看：运行时直接派一个只有读文件、搜索、列目录工具的子 agent（不经对话的决定）。
  async runBotProactiveCheck(bot) {
    // 工具范围里连只读工具都没有时没法看，不开。
    const tools = botProactiveTools(bot)
    if (!tools.length) return null
    const value = await this.getOrCreateSession(bot.threadSessionId)
    if (!value?.conversationOnly || !value.multiAgentRuntime) return null
    const instructions = await this.botAgentInstructions(bot)
    const agent = await value.multiAgentRuntime.spawn({
      // 不写标题：界面按任务 id 显示本地化的“只读巡看”。
      taskName: BOT_PROACTIVE_TASK_NAME,
      ...(instructions ? { instructions } : {}),
      message: botProactiveTask(bot),
      tier: 'fast',
      toolNames: tools,
      parentLimit: bot.maxParallelAgents || BOT_MAX_PARALLEL_AGENTS,
      ...(this.botSkills ? { skillPaths: [this.botSkills.dir(bot.id)] } : {}),
    })
    this.botProactiveAgents ||= new Set()
    this.botProactiveAgents.add(agent.id)
    return agent
  }

  // 巡看做完交回对话的那句：在普通的完成通知后面加一句“写成安静的建议，不开活”。
  botCompletionPromptFor(agent) {
    return this.botProactiveAgents?.has(agent.id)
      ? `${agentCompletionPrompt(agent)}\n\n${BOT_PROACTIVE_NOTE}`
      : null
  }

  // 每日简报：一条隐藏消息说进对话（过去一天做了什么、今天排了什么），对话写出简报；这一轮不开活。
  async sendBotBrief(bot, now = Date.now()) {
    const sessionId = bot.threadSessionId
    const jobs = (this.multiAgents.list(sessionId) || []).filter(
      (agent) => now - Date.parse(agent.startedAt || '') < 24 * 60 * 60 * 1000,
    )
    const schedules = (this.schedules?.getState?.().tasks || [])
      .filter((task) => task.targetType === 'bot' && task.botId === bot.id && task.enabled)
      .map((task) => `${task.time || ''} ${task.botMessage || task.name || ''}`.trim())
    void this.promptFromChannel({
      sessionId,
      message: botBriefPrompt({ bot, jobs, schedules }),
      unattended: true,
    }).catch(() => console.warn('[bots] Could not write the daily brief.'))
  }

  startBotProactiveTimer() {
    if (this.botProactiveTimer || !this.capabilities.features.bots) return
    this.botProactiveTimer = setInterval(() => {
      void this.botProactiveTick().catch(() => console.warn('[bots] Proactive check tick failed.'))
    }, 60_000)
    this.botProactiveTimer.unref?.()
  }

  // 给另一个 Bot 发话（message 决定）：对方要存在、开着、不是自己；客气话不发。
  // 这一轮是对方说进来的话时，接着同一条链（跳数加一）；否则开一条新链，时间线里一条“在等它回话”。
  async sendBotMessage(bot, decision, trigger) {
    const key = botNameKey(decision.bot)
    const target = this.bots
      .list()
      .find((item) => item.id === decision.bot || botNameKey(item.name) === key)
    if (!target || target.id === bot.id) throw new Error('No such bot.')
    if (!target.enabled) throw new Error(`${target.name} is paused.`)
    if (isCourtesyMessage(decision.text)) return { type: 'message', skipped: 'courtesy' }
    const continuing = trigger?.message?.from === target.id ? trigger.message : null
    const chain = continuing?.chain || newBotChainId()
    // 发给第三个 Bot 时开一条新的来往（时间线上各记各的），但跳数接着这一轮的来由往下数：
    // 不然 A→B、B→C、C→A 每一句都是“新链第 1 跳”，6 跳的上限永远到不了。
    const hop = continuing
      ? Math.max(continuing.hop, this.botLinks.get(chain)?.hops || 0) + 1
      : this.botTurnOriginHop(trigger) + 1
    if (!continuing) this.botLinks.open({ chain, from: bot.id, to: target.id, text: decision.text })
    const sent = await this.deliverBotMessage(bot, target, {
      text: decision.text,
      chain,
      hop,
      reply: decision.expectsReply,
    })
    if (sent.ok && continuing) this.botLinks.advance(chain, { hop, waiting: decision.expectsReply })
    return { type: 'message', bot: target.id, chain, ...(sent.ok ? {} : { skipped: sent.reason }) }
  }

  // 这一轮是从 Bot 之间的哪一跳来的：别的 Bot 说进来的话，或替别的 Bot 做的活做完了；都不是就是 0（你说的话、定时）。
  botTurnOriginHop(trigger) {
    let hop = 0
    const message = trigger?.message
    if (message) hop = Math.max(message.hop, this.botLinks.get(message.chain)?.hops || 0)
    for (const agentId of trigger?.completedAgents || []) {
      const request = this.botAgentRequests?.get(agentId)
      if (request) hop = Math.max(hop, request.hop, this.botLinks.get(request.chain)?.hops || 0)
    }
    return hop
  }

  // 这一轮说完后把话交回去：别的 Bot 在等回话的，回复发给它；替别的 Bot 做的活做完了，汇报发给它。
  // 交回去的话不再要求回话（否则两边会一直回下去）；客气话不交。
  async forwardBotReplies(bot, value, trigger) {
    if (!this.botLinks || !trigger.reply) return []
    const targets = new Map()
    if (trigger.message?.reply) targets.set(trigger.message.chain, trigger.message)
    for (const agentId of trigger.completedAgents) {
      const request = this.botAgentRequests?.get(agentId)
      if (!request) continue
      this.botAgentRequests.delete(agentId)
      targets.set(request.chain, {
        from: request.fromBotId,
        chain: request.chain,
        hop: request.hop,
      })
    }
    const sent = []
    for (const request of targets.values()) {
      const target = this.bots.get(request.from)
      // 还有替这条链在做的活：那边接着等最后的结果。
      const pending = [...(this.botAgentRequests?.values() || [])].some(
        (item) => item.chain === request.chain,
      )
      if (!target || isCourtesyMessage(trigger.reply)) {
        this.botLinks.advance(request.chain, { hop: request.hop, waiting: pending })
        continue
      }
      // 链上已经走过的跳数为准（先回过一句、后来又交结果时，结果是再下一跳）。
      const hop = Math.max(request.hop, this.botLinks.get(request.chain)?.hops || 0) + 1
      const result = await this.deliverBotMessage(bot, target, {
        text: trigger.reply.slice(0, 4_000),
        chain: request.chain,
        hop,
        reply: false,
      })
      if (result.ok) this.botLinks.advance(request.chain, { hop, waiting: pending })
      sent.push({
        bot: target.id,
        chain: request.chain,
        ...(result.ok ? {} : { skipped: result.reason }),
      })
    }
    return sent
  }

  // 把一句话投进对方的对话（标着来自谁，转录里显示发话 Bot 的头像与名字）。超过跳数或太频繁时停下，
  // 两边的时间线里都写着“停在这里了”。对方正在说话时排在它这一轮之后。
  async deliverBotMessage(from, to, { text, chain, hop, reply }) {
    const admitted = this.botLinks.admit({ from: from.id, to: to.id, chain, hop })
    if (!admitted.ok) {
      if (!this.botLinks.get(chain)) this.botLinks.open({ chain, from: from.id, to: to.id, text })
      this.botLinks.stop(chain, admitted.reason)
      return admitted
    }
    const context = botMessageContext({ fromName: from.name, from: from.id, chain, hop, reply })
    await this.messageBot(to.id, {
      text: `${text}${ATTACHMENT_MARKER}${context}`,
      source: { kind: 'bot', botId: from.id },
    })
    return admitted
  }

  // 会话时间线里的 Bot 来往（它发起的；别人找它而停下来的）。不是 Bot 的会话为空。
  listSessionLinks(sessionId) {
    const bot = this.botForSession(String(sessionId || ''))
    return bot && this.botLinks ? this.botLinks.listForBot(bot.id) : []
  }

  // 子 agent 做完：复盘这件活（独立请求，失败不影响汇报）——会反复做的写成技能，学到新东西的更新技能。
  // 同一个 Bot 的复盘按顺序跑，两件同类活同时做完时第二次复盘看得到第一次写的技能。
  botAgentFinished(agent) {
    // 巡看不是谁交代的活，不复盘成技能。
    if (agent?.status !== 'completed' || !this.botSkills) return null
    if (this.botProactiveAgents?.has(agent.id)) return null
    const bot = this.botForSession(agent.parentSessionId)
    if (!bot) return null
    this.botReviewChains ||= new Map()
    const previous = this.botReviewChains.get(bot.id) || Promise.resolve()
    const next = previous
      .then(() => this.reviewBotJob(bot, agent))
      .catch(() => console.warn('[bots] Could not finish the review.'))
      .finally(() => {
        if (this.botReviewChains.get(bot.id) === next) this.botReviewChains.delete(bot.id)
      })
    this.botReviewChains.set(bot.id, next)
    return next
  }

  async reviewBotJob(bot, agent) {
    const conversation = this.sessions.get(bot.threadSessionId)?.session?.model
    const model =
      conversation ||
      (
        await this.providerPreferences.resolveSubagentModel(
          bot.model.ref ? { modelRef: bot.model.ref } : { tier: bot.model.tier },
        )
      ).model
    if (!model) return null
    const [skills, memory] = await Promise.all([
      this.botSkills.list(bot.id),
      this.botMemory.list(bot.id).catch(() => []),
    ])
    const result = await requestBotText({
      modelRuntime: this.modelRuntime,
      model,
      text: botReviewInstruction({
        title: agent.title || agent.taskName,
        task: agent.message,
        output: agent.fullOutput || agent.output,
        skills,
        corrections: memory
          .filter((entry) => entry.source === 'correction')
          .map((entry) => entry.text),
      }),
    })
    if (result.usage) {
      await this.recordUsage(
        localDayKey(),
        `bot-review:${bot.id}:${randomUUID()}`,
        result.usage,
      ).catch(() => {})
    }
    const skill = result.text === null ? null : parseBotSkillReview(result.text)
    return skill ? this.botSkills.write(bot.id, skill) : null
  }

  // 设置页“它学会的”：看、删。技能只由复盘写入，这里不手写。
  listBotSkills(botId) {
    return this.botSkills.list(this.requireBot(botId).id)
  }

  getBotSkill(botId, name) {
    return this.botSkills.get(this.requireBot(botId).id, name)
  }

  deleteBotSkill(botId, name) {
    return this.botSkills.remove(this.requireBot(botId).id, name)
  }

  // 删除 Bot 时一并删掉指向它的定时任务（README「Bot」）。它的会话留着，变成一个普通会话。
  async deleteBot(id) {
    const removed = await this.bots.remove(id)
    if (removed) await this.schedules?.deleteForBot?.(id)
    // 它自己的记忆一并删掉（会话留着，变成普通会话）。
    if (removed) await this.botMemory?.removeBot(id)
    if (removed) this.botLinks?.removeBot(id)
    return removed
  }

  // 设置页的记忆：看、加、改、删。Bot 不存在时报 404。
  requireBot(botId) {
    const bot = this.bots.get(botId)
    if (!bot)
      throw Object.assign(new Error('Bot not found.'), { code: 'bot_not_found', statusCode: 404 })
    return bot
  }

  listBotMemory(botId) {
    return this.botMemory.list(this.requireBot(botId).id)
  }

  addBotMemory(botId, text) {
    return this.botMemory.remember(this.requireBot(botId).id, text, 'manual')
  }

  editBotMemory(botId, entryId, text) {
    return this.botMemory.edit(this.requireBot(botId).id, entryId, text)
  }

  async forgetBotMemory(botId, entryId) {
    return this.botMemory.forget(this.requireBot(botId).id, { id: entryId })
  }

  // 渠道与定时任务用的 Bot 接口（B3）：都在 Bots 实验功能打开时才生效。
  botChannelAdapter() {
    return {
      enabled: () => Boolean(this.capabilities.features.bots && this.isBetaFeatureEnabled('bots')),
      list: () => this.bots?.list() || [],
      get: (id) => this.bots?.get(id) || null,
      activity: (botId) => {
        const bot = this.bots?.get(botId)
        return bot ? this.botActivity(bot) : null
      },
      message: (botId, input) => this.messageBot(botId, input),
      act: (botId, action) => this.botChannelAction(botId, action),
    }
  }

  // Bot 能推送的群：它的聊天规则里的群（规则写了群就是那些，写“所有群”就是这个平台上见过的群）。
  // 选哪个：给了就按 ID 或群名找；没给时用这一轮来自的群，或只有一个时就是它。微信没有群。
  botGroupTarget(bot, { group, platform, origin } = {}) {
    const groups = new Map()
    for (const rule of bot.channels || []) {
      if (rule.chatType !== 'group' || rule.platform === 'weixin') continue
      if (platform && rule.platform !== platform) continue
      const known = this.channels?.knownGroups(rule.platform) || []
      const ids = rule.peerIds || known.map((item) => item.peerId)
      for (const peerId of ids) {
        const title = known.find((item) => item.peerId === peerId)?.title || ''
        groups.set(`${rule.platform}:${peerId}`, { platform: rule.platform, peerId, title })
      }
    }
    const all = [...groups.values()]
    const wanted = String(group ?? '').trim()
    const named = wanted
      ? all.filter(
          (item) =>
            item.peerId === wanted ||
            `${item.platform}:${item.peerId}` === wanted ||
            (item.title && item.title.toLowerCase() === wanted.toLowerCase()),
        )
      : origin?.chatType === 'group'
        ? all.filter((item) => item.platform === origin.platform && item.peerId === origin.peerId)
        : all
    if (named.length === 1) return named[0]
    const list = all.length
      ? all
          .map((item) => `${item.platform}:${item.peerId}${item.title ? ` (${item.title})` : ''}`)
          .join(', ')
      : ''
    if (!all.length)
      throw new Error(
        'Not sent: your chat rules include no group you can post to (WeChat has no groups; with "all groups", only groups where someone has talked since are known). The user can add one under your Chats settings.',
      )
    throw new Error(
      wanted
        ? `Not sent: "${wanted}" is not one of your groups: ${list}.`
        : `Not sent: say which group. Your groups: ${list}.`,
    )
  }

  // B5：从聊天里对 Bot 说的话，回复回到同一个聊天。回推失败只记日志，不影响会话。
  notifyBotChat(source, text) {
    if (!text || source?.kind !== 'channel' || !source.platform || !source.peerId) return
    if (!this.capabilities.features.bots || !this.isBetaFeatureEnabled('bots')) return
    void this.channels
      ?.sendToChat(source.platform, source.peerId, text)
      .catch((error) =>
        console.warn(
          '[bots] Could not send to the channel:',
          error instanceof Error ? error.message : error,
        ),
      )
  }

  // Bot 的会话里出现审批或提问时，如果这一轮是从聊天里叫起来的，就告诉那个聊天。
  botSessionEvent(sessionId, event, data) {
    if (event !== 'permission_request' && event !== 'question_request') return
    const source = this.botChatOrigins.get(sessionId)
    const bot = source ? this.botForSession(sessionId) : null
    if (!bot) return
    this.notifyBotChat(
      source,
      event === 'permission_request'
        ? botApprovalText(bot.name, data)
        : botQuestionText(bot.name, data),
    )
  }

  // 聊天里的处理命令：/名字 批准 | 拒绝。发送者已按 Bot 的名单校验过。
  async botChannelAction(botId, action) {
    const bot = this.bots.get(botId)
    if (!bot) return botChannelActionReply('', 'no_bot')
    const sessionId = bot.threadSessionId
    const approval = sessionId ? this.botWaiting(sessionId).approval : null
    if (!approval) return botChannelActionReply(bot.name, 'nothing')
    if (action === 'approve' && approval.severity === 'critical')
      return botChannelActionReply(bot.name, 'critical')
    const resolution = this.resolveToolApproval(sessionId, approval.id, action === 'approve')
    if (!resolution?.found) return botChannelActionReply(bot.name, 'handled')
    return botChannelActionReply(bot.name, action === 'approve' ? 'approved' : 'denied')
  }

  // 远程会话的执行节点（B6）。节点不存在时也返回执行器，调用会以“离线”失败，绝不悄悄退回在本机执行。
  executorFor(environmentId) {
    if (isSshMachineId(environmentId)) return this.sshMachines.executor(environmentId)
    const environment = this.environments.get(environmentId) || {
      id: environmentId,
      name: '',
      capabilities: {},
    }
    return new NodeExecutor({ hub: this.executorHub, environment })
  }

  // Bot 的电脑上一层子文件夹（设置里点选目录用）。本机走与目录面板同一个列表，执行节点转过去问它。
  async listEnvironmentFolders(environmentId, path, { signal } = {}) {
    if (environmentId === 'local')
      return listWorkspaceFolders({ path }, { fallback: this.cwd || '' })
    if (isSshMachineId(environmentId) || !this.environments.get(environmentId))
      throw new EnvironmentError('environment_not_found', 'Machine not found.', 404)
    return this.executorFor(environmentId).listFolders(path, { signal })
  }

  listEnvironments() {
    const bots = this.bots.list()
    const usedBy = (id) =>
      bots
        .filter((bot) => (bot.workspace.environmentId || 'local') === id)
        .map((bot) => ({ id: bot.id, name: bot.name }))
    return {
      environments: [
        {
          id: 'local',
          kind: 'local',
          name: '',
          online: true,
          roots: [],
          capabilities: { shell: true },
          usedBy: usedBy('local'),
        },
        ...this.environments.list().map((environment) => ({
          ...environment,
          online: this.executorHub.online(environment.id),
          usedBy: usedBy(environment.id),
        })),
      ],
    }
  }

  // 远程机器：SSH 和对方连过来（执行节点）在界面和 Agent 那边是一回事，只是连法不同。
  listMachines() {
    return [
      ...this.sshMachines.list().map((machine) => ({
        id: machine.id,
        kind: 'ssh',
        name: machine.name,
        target: machine.target,
        port: machine.port,
        defaultCwd: machine.defaultCwd,
        online: machine.lastCheck?.ok === true,
        lastCheck: machine.lastCheck,
        roots: [],
      })),
      ...this.environments.list().map((environment) => ({
        id: environment.id,
        kind: 'node',
        name: environment.name,
        defaultCwd: environment.roots[0] || '',
        online: this.executorHub.online(environment.id),
        lastCheck: null,
        roots: environment.roots,
      })),
    ]
  }

  machineSummary(id) {
    const machine = isSshMachineId(id) ? this.sshMachines.get(id) : this.environments.get(id)
    return { id, kind: isSshMachineId(id) ? 'ssh' : 'node', name: machine?.name || '' }
  }

  async removeMachine(id) {
    if (isSshMachineId(id)) return this.sshMachines.remove(id)
    return this.removeEnvironment(id)
  }

  // 对方机器上的目录：解析成绝对路径并确认能用。选目录时和建会话时都走这里。
  async resolveMachineFolder(id, input) {
    if (isSshMachineId(id)) {
      if (!this.sshMachines.get(id))
        throw new ExecutorError('machine_not_found', 'Machine was removed.')
      const cwd = await this.sshMachines.executor(id).resolveDirectory(input)
      await this.sshMachines.recordReachable(id)
      return cwd
    }
    const environment = this.environments.get(id)
    if (!environment) throw new ExecutorError('machine_not_found', 'Machine was removed.')
    if (!this.executorHub.online(id))
      throw new ExecutorError(
        'machine_offline',
        'Machine is offline: run npx pisper executor on it.',
      )
    const cwd = String(input ?? '').trim() || environment.roots[0] || ''
    if (!pathWithinRoots(cwd, environment.roots))
      throw new ExecutorError(
        'machine_folder_outside_roots',
        `Folder is outside the allowed roots: ${environment.roots.join(', ')}`,
      )
    return cwd
  }

  // 新会话落在远程机器上：大脑这边只有占位目录，工具在对方的 cwd 里执行。
  // 建会话前先确认目录真的在，失败时直接告诉用户，不等到第一个工具才发现。
  async createMachineSession(name, input) {
    const id = String(input?.id || '')
    const cwd = await this.resolveMachineFolder(id, input?.cwd)
    return this.sessionLifecycle.createSession(name, await this.executorSessionDirectory(id), {
      metadata: { remoteExecutor: { environmentId: id, cwd } },
    })
  }

  // 只改会话元数据；调用方随后用 setSessionCwd 重建会话，新工具才生效。
  async setSessionExecutor(id, executor) {
    const meta = { ...(this.sessionMeta[id] || {}) }
    if (executor) meta.remoteExecutor = { environmentId: executor.environmentId, cwd: executor.cwd }
    else delete meta.remoteExecutor
    this.sessionMeta[id] = meta
    await this.saveSessionMeta()
  }

  // 远程会话在大脑这边的占位目录：每台电脑一个，里面不放任何东西。
  async executorSessionDirectory(environmentId) {
    const directory = join(this.dataDir, 'executor-sessions', environmentId)
    await mkdir(directory, { recursive: true })
    return directory
  }

  createEnvironmentPairing() {
    return this.environments.createPairing()
  }

  async removeEnvironment(id) {
    const inUseBy = this.bots
      .list()
      .filter((bot) => bot.workspace.environmentId === id)
      .map((bot) => bot.name)
    const removed = await this.environments.remove(id, { inUseBy })
    if (removed) this.executorHub.disconnect(id)
    return removed
  }

  pairExecutor(input) {
    return this.environments.pair(input)
  }

  authenticateExecutor(bearer) {
    return this.environments.authenticate(bearer)
  }

  async pollExecutor(environment, report, signal) {
    await this.environments.report(environment.id, report)
    return this.executorHub.poll(environment.id, { signal })
  }

  deliverExecutorResults(environment, results) {
    return this.executorHub.deliver(environment.id, results)
  }

  // Bot 的会话在等的东西：最早一条待审批（与看板卡片同一份摘要）与最早一组待回答的问题。
  botWaiting(sessionId) {
    const approval = this.permissions
      .getPending(sessionId)
      .toSorted((left, right) => String(left.createdAt).localeCompare(String(right.createdAt)))
      .map(projectApproval)
      .find(Boolean)
    const question = projectQuestionSummary(this.questions?.getPending(sessionId) || [])
    return { approval: approval ?? null, question }
  }

  async updateCompactionPreference(input) {
    const requested = Number(input?.thresholdPercent)
    if (
      !Number.isFinite(requested) ||
      requested < MIN_COMPACTION_THRESHOLD_PERCENT ||
      requested > MAX_COMPACTION_THRESHOLD_PERCENT
    ) {
      throw pisperError(
        'invalid',
        'config',
        `compaction threshold must be ${MIN_COMPACTION_THRESHOLD_PERCENT}-${MAX_COMPACTION_THRESHOLD_PERCENT}%`,
      )
    }
    const thresholdPercent = normalizeCompactionThresholdPercent(requested)
    const appConfig = await readJson(this.appConfigPath, {})
    await writeJsonAtomic(this.appConfigPath, {
      ...appConfig,
      compactionThresholdPercent: thresholdPercent,
    })
    this.compactionThresholdPercent = thresholdPercent
    this.sessionContextUsageCache.clear()
    this.streamProjection.invalidateAllUsage()
    return this.getCompactionPreference()
  }

  getNotificationSettings() {
    return this.notificationSettings.getState()
  }

  updateBrowserNotifications(input) {
    return this.notificationSettings.updateBrowser(input)
  }

  saveNotificationTemplate(event, platform, input) {
    return this.notificationSettings.updateTemplate(event, platform, input)
  }

  testNotificationTemplate(event, platform) {
    return this.notificationSettings.testTemplate(event, platform)
  }

  getBrowserNotificationEvents(after) {
    return this.notificationSettings.getBrowserEvents(after)
  }

  async getSchedules() {
    const config = await this.getConfig()
    const notificationSettings = await this.notificationSettings.getState()
    return {
      ...this.schedules.getState(),
      defaultCwd: this.cwd,
      workflows: this.workflows
        .getState()
        .workflows.filter((workflow) => workflow.status === 'published')
        .map(({ id, name, description, revision, inputs }) => ({
          id,
          name,
          description,
          revision,
          inputs,
        })),
      models: config.providers
        .filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured)
        .flatMap((provider) =>
          provider.models
            .filter((model) => model.kind === 'chat')
            .map((model) => ({
              provider: provider.id,
              model: model.id,
              label: `${provider.name} / ${model.name}`,
            })),
        ),
      notificationTargets: {
        browser: { enabled: notificationSettings.browser.enabled },
        feishu: { enabled: Boolean(notificationSettings.connections.feishu?.enabled) },
        weixin: { enabled: Boolean(notificationSettings.connections.weixin?.enabled) },
        qq: { enabled: Boolean(notificationSettings.connections.qq?.enabled) },
        telegram: { enabled: Boolean(notificationSettings.connections.telegram?.enabled) },
      },
    }
  }

  async createSchedule(input) {
    const task = await this.schedules.create(input)
    return { task, state: await this.getSchedules() }
  }

  async updateSchedule(id, input) {
    const task = await this.schedules.update(id, input)
    return task ? { task, state: await this.getSchedules() } : null
  }

  deleteSchedule(id) {
    return this.schedules.remove(id)
  }

  async runSchedule(id) {
    const task = await this.schedules.runNow(id)
    return task ? { started: true, task } : null
  }

  async getWorkflows() {
    const config = await this.getConfig()
    const notificationSettings = await this.notificationSettings.getState()
    const skills = await this.skills.dashboard({ cwd: this.cwd })
    return {
      ...this.workflows.getState(),
      skills: skills.skills
        .filter((skill) => skill.enabled && skill.command)
        .map((skill) => ({ id: skill.id, name: skill.name, description: skill.description })),
      cwd: this.cwd,
      models: config.providers
        .filter((provider) => provider.type !== 'visual' && provider.enabled && provider.configured)
        .flatMap((provider) =>
          provider.models
            .filter((model) => model.kind === 'chat')
            .map((model) => ({
              provider: provider.id,
              model: model.id,
              label: `${provider.name} / ${model.name}`,
            })),
        ),
      notificationTargets: {
        browser: { enabled: notificationSettings.browser.enabled },
        feishu: { enabled: Boolean(notificationSettings.connections.feishu?.enabled) },
        weixin: { enabled: Boolean(notificationSettings.connections.weixin?.enabled) },
        qq: { enabled: Boolean(notificationSettings.connections.qq?.enabled) },
        telegram: { enabled: Boolean(notificationSettings.connections.telegram?.enabled) },
      },
    }
  }

  async createWorkflow(input) {
    const notificationSettings = await this.notificationSettings.getState()
    const workflow = await this.workflows.create(
      filterWorkflowNotificationTargets(input, enabledNotificationTargets(notificationSettings)),
    )
    return { workflow, state: await this.getWorkflows() }
  }

  async updateWorkflow(id, input) {
    const notificationSettings = await this.notificationSettings.getState()
    const workflow = await this.workflows.update(
      id,
      filterWorkflowNotificationTargets(input, enabledNotificationTargets(notificationSettings)),
    )
    return workflow ? { workflow, state: await this.getWorkflows() } : null
  }

  deleteWorkflow(id) {
    return this.workflows.remove(id)
  }

  async runWorkflow(id, input = {}) {
    const run = await this.workflows.runNow(id, input)
    return run ? { started: true, run } : null
  }

  getWorkflowRun(id) {
    return this.workflows.getRun(id)
  }

  async retryWorkflowRun(id) {
    const run = await this.workflows.retryRun(id)
    return run ? { started: true, run } : null
  }

  async resolveWorkflowApproval(runId, nodeId, input) {
    const run = await this.workflows.resolveApproval(runId, nodeId, input?.approved, input?.comment)
    return run ? { resolved: true, run } : null
  }

  async duplicateWorkflow(id, input) {
    const workflow = await this.workflows.duplicate(id, input)
    return workflow ? { workflow, state: await this.getWorkflows() } : null
  }

  exportWorkflow(id) {
    return this.workflows.exportWorkflow(id)
  }

  async importWorkflow(input) {
    const workflow = await this.workflows.importWorkflow(input)
    return { workflow, state: await this.getWorkflows() }
  }

  getSessionWorkflowRuns(sessionId) {
    return { runs: this.workflows.getState({ sessionId }).runs }
  }

  async stopWorkflowRun(id) {
    const run = await this.workflows.stop(id)
    return run ? { stopping: true, run } : null
  }

  notifyChannels(event, data, options) {
    return this.notificationSettings.notify(event, data, options)
  }

  // 全局关闭流程：清理定时器、后台服务与挂起的写盘任务，保证进程可干净退出。
  async dispose() {
    // 先断开看板事件流：关闭期间不再为在线客户端生成卡片增量。
    this.boardEvents?.dispose()
    this.sessionCards?.dispose()
    await this.memoryCapture.dispose()
    this.auxiliaryTitles?.dispose()
    if (this.sessionRuntimeSweepTimer) clearInterval(this.sessionRuntimeSweepTimer)
    this.sessionRuntimeSweepTimer = null
    if (this.botProactiveTimer) clearInterval(this.botProactiveTimer)
    this.botProactiveTimer = null
    await this.sideChats?.dispose()
    for (const timer of this.agentWakeupTimers.values()) clearTimeout(timer)
    this.agentWakeupTimers.clear()
    this.providerModelDiscovery.abort?.()
    await this.providerModelRefreshPromise?.catch(() => {})
    await this.providerModelCatalog.dispose()
    await this.modelMetadata.dispose()
    await this.workflows.dispose()
    await this.gameAssets?.dispose()
    await this.imageAssets?.dispose()
    await this.workflowImageOperations?.dispose()
    await this.gameAssetImageOperations?.dispose()
    await this.agentImageOperations?.dispose()
    await this.workflowImageProcessor?.dispose()
    await this.spriteEngines?.dispose()
    await this.workflowMedia?.dispose()
    await this.gameAssetMedia?.dispose()
    await this.agentImageMedia?.dispose()
    this.executorHub?.dispose()
    await this.schedules.dispose()
    await this.channels.dispose()
    await this.goals.pauseAllActive()
    await this.teamWorkflows?.pauseActive?.()
    await this.multiAgents.dispose()
    await this.browserAutomation.dispose()
    this.desktopComputer.dispose()
    this.mobileOperations.dispose()
    this.permissions.dispose()
    this.questions?.dispose()
    await this.sessionQueue?.dispose()
    await this.disposeSessions()
    await this.sandbox?.close?.()
    this.pendingSessions.clear()
    await this.mcp.dispose()
    this.toolPlugins.dispose()
    await this.memory.dispose()
    await Promise.allSettled([
      this.sessionMetaWrite,
      this.usageWrite,
      this.assetReconcile,
      this.assetWrite,
    ])
  }

  async savePlugins(input) {
    const [current, visible] = await Promise.all([this.toolPlugins.getState(), this.getPlugins()])
    const visibleToolNames = new Set(visible.tools.map((tool) => tool.id))
    const preservedToolNames = current.enabledTools.filter((tool) => !visibleToolNames.has(tool))
    await this.toolPlugins.saveState({
      ...input,
      enabledTools: [...new Set([...(input?.enabledTools || []), ...preservedToolNames])],
    })
    this.invalidateSessionRuntimes()
    return await this.getPlugins()
  }

  inspectPlugin(input) {
    return this.toolPlugins.inspect(input?.path)
  }

  async installPlugin(input) {
    const result = await this.toolPlugins.install(input?.inspectionId)
    this.invalidateSessionRuntimes()
    return result
  }

  async setPluginEnabled(id, enabled) {
    const result = await this.toolPlugins.setPluginEnabled(id, enabled)
    this.invalidateSessionRuntimes()
    return result
  }

  async setPluginCapabilityEnabled(id, name, enabled) {
    const result = await this.toolPlugins.setCapabilityEnabled(id, name, enabled)
    this.invalidateSessionRuntimes()
    return result
  }

  async uninstallPlugin(id) {
    const result = await this.toolPlugins.uninstall(id)
    this.invalidateSessionRuntimes()
    return result
  }

  getMcpDashboard({ refresh = true } = {}) {
    return this.mcp.getDashboard({ refresh })
  }

  async createMcpServer(input) {
    const result = await this.mcp.add(input)
    this.invalidateSessionRuntimes()
    return result
  }

  async updateMcpServer(id, input) {
    const result = await this.mcp.update(id, input)
    if (result) this.invalidateSessionRuntimes()
    return result
  }

  async deleteMcpServer(id) {
    const deleted = await this.mcp.remove(id)
    if (deleted) this.invalidateSessionRuntimes()
    return deleted
  }

  async testMcpServer(id) {
    return this.mcp.test(id)
  }

  async setMcpToolEnabled(id, toolName, enabled) {
    const result = await this.mcp.setToolEnabled(id, toolName, enabled)
    if (result) this.invalidateSessionRuntimes()
    return result
  }

  async sessionGitCwd(id) {
    return this.sessionWorkspaceCwd(id)
  }

  async getSessionGitChanges(id) {
    return this.gitChanges.getChanges(await this.sessionGitCwd(id))
  }

  async commitSessionGitChanges(id, message) {
    if (this.sessionRunIsActive(id)) throw pisperError('busy', 'session')
    return this.gitChanges.commit(await this.sessionGitCwd(id), message)
  }

  async pushSessionGitChanges(id) {
    return this.gitChanges.push(await this.sessionGitCwd(id))
  }

  async revertSessionGitChanges(id) {
    if (this.sessionRunIsActive(id)) throw pisperError('busy', 'session')
    return this.gitChanges.revert(await this.sessionGitCwd(id))
  }

  async getSessionVcsChanges(id) {
    return this.vcsChanges.getChanges(await this.sessionGitCwd(id))
  }

  // 单文件差异：会话文件 chip「查看改动」的数据源；
  // 路径必须落在会话工作区内，避免借会话读取任意目录的 diff。
  // 非 VCS 工作区回退到会话内修改前快照，保证无 Git/SVN 也能预览。
  // preferSession：文件 chip 的撤销恢复的是快照，有快照时 diff 也用它，看到的和撤销的才是同一份改动。
  async getSessionFileDiff(id, filePath, { preferSession = false } = {}) {
    const cwd = await this.sessionGitCwd(id)
    const target = resolve(cwd, String(filePath || ''))
    const relativePath = relative(cwd, target)
    if (!String(filePath || '') || relativePath.startsWith('..') || isAbsolute(relativePath))
      throw pisperError('forbidden', 'path', 'outside the session workspace')
    const relPath = relativePath.replace(/\\/g, '/')
    const snapshotDiff = async () => {
      const snapshot = await this.getFileChangesService().diff(id, cwd, relPath)
      if (!snapshot.found || !snapshot.diff.trim()) return null
      return {
        isRepo: false,
        vcs: '',
        diff: snapshot.diff,
        diffTruncated: snapshot.diffTruncated,
        source: 'snapshot',
        canRevert: true,
      }
    }
    if (preferSession) {
      const session = await snapshotDiff()
      if (session) return session
    }
    const vcs = await this.vcsChanges.getFileDiff(cwd, target)
    if (vcs.isRepo && vcs.diff?.trim())
      return { ...vcs, source: vcs.vcs || 'vcs', canRevert: false }
    return (await snapshotDiff()) || { ...vcs, source: vcs.isRepo ? vcs.vcs || 'vcs' : '' }
  }

  getFileChangesService() {
    this.fileChanges ||= new SessionFileChangesService({ dataDir: this.dataDir })
    return this.fileChanges
  }

  // 会话文件变更清单（无 VCS 审批视图的数据源）。
  async getSessionFileChanges(id) {
    const cwd = await this.sessionWorkspaceCwd(id)
    return { cwd, ...(await this.getFileChangesService().list(id, cwd)) }
  }

  // 会话目录的轻量改动摘要只来自本会话写工具快照，不混用工作区 Git/SVN 状态。
  async getSessionChangeSummary(id) {
    const cwd = await this.sessionWorkspaceCwd(id)
    return this.getFileChangesService().summary(id, cwd)
  }

  async getSessionFileChangeDiff(id, filePath) {
    const cwd = await this.sessionWorkspaceCwd(id)
    const relPath = this.fileChangeRelPath(cwd, filePath)
    return this.getFileChangesService().diff(id, cwd, relPath)
  }

  async revertSessionFileChanges(id, filePath) {
    if (this.sessionRunIsActive(id)) throw pisperError('busy', 'session')
    const cwd = await this.sessionWorkspaceCwd(id)
    const relPath = filePath ? this.fileChangeRelPath(cwd, filePath) : ''
    const result = await this.getFileChangesService().revert(id, cwd, relPath || undefined)
    // HTTP 面保持清单响应平铺，前端不需要了解服务层的内部包装。
    return { ...result.files, reverted: result.reverted }
  }

  async approveSessionFileChanges(id, filePath) {
    const cwd = await this.sessionWorkspaceCwd(id)
    const relPath = filePath ? this.fileChangeRelPath(cwd, filePath) : ''
    const result = await this.getFileChangesService().approve(id, cwd, relPath || undefined)
    return result.files
  }

  // 文件变更接口统一的路径校验：必须落在会话工作区内。
  fileChangeRelPath(cwd, filePath) {
    const target = resolve(cwd, String(filePath || ''))
    const relativePath = relative(cwd, target)
    if (!String(filePath || '') || relativePath.startsWith('..') || isAbsolute(relativePath))
      throw pisperError('forbidden', 'path', 'outside the session workspace')
    return relativePath.replace(/\\/g, '/')
  }

  async commitSessionVcsChanges(id, message) {
    if (this.sessionRunIsActive(id)) throw pisperError('busy', 'session')
    return this.vcsChanges.commit(await this.sessionGitCwd(id), message)
  }

  async pushSessionVcsChanges(id) {
    return this.vcsChanges.push(await this.sessionGitCwd(id))
  }

  async revertSessionVcsChanges(id) {
    if (this.sessionRunIsActive(id)) throw pisperError('busy', 'session')
    return this.vcsChanges.revert(await this.sessionGitCwd(id))
  }

  async getSkillsDashboard(sessionId = '') {
    return this.skills.dashboard({ cwd: await this.sessionWorkspaceCwd(sessionId) })
  }

  extensionMarketplace(input) {
    return this.skills.extensionMarketplace(input)
  }

  async getExtensionDashboard(sessionId = '') {
    return this.skills.extensionDashboard({ cwd: await this.sessionWorkspaceCwd(sessionId) })
  }

  async installExtension(input, sessionId = '') {
    const result = await this.skills.installExtension(input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    await this.refreshSessionRuntimes()
    return result
  }

  async removeExtension(source, scope = 'user', sessionId = '') {
    const removed = await this.skills.removeExtension(source, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
      scope,
    })
    if (removed) await this.refreshSessionRuntimes()
    return removed
  }

  async installSkill(input, sessionId = '') {
    const result = await this.skills.install(input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    this.invalidateSessionRuntimes()
    return result
  }

  async updateSkill(id, input, sessionId = '') {
    const result = await this.skills.update(id, input, {
      cwd: await this.sessionWorkspaceCwd(sessionId),
    })
    if (result) this.invalidateSessionRuntimes()
    return result
  }

  async deleteSkill(id, sessionId = '') {
    const deleted = await this.skills.remove(id, { cwd: await this.sessionWorkspaceCwd(sessionId) })
    if (deleted) this.invalidateSessionRuntimes()
    return deleted
  }

  async reloadSkills(sessionId = '') {
    this.invalidateSessionRuntimes()
    this.skills.invalidateDashboardCache()
    return this.skills.dashboard({ cwd: await this.sessionWorkspaceCwd(sessionId), force: true })
  }

  async getProviderDiscovery() {
    return this.providerPreferences.getProviderDiscovery()
  }

  async importLocalProviders() {
    return this.providerPreferences.importLocalProviders()
  }

  async importDiscoveredProvider(discoveryId) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.importDiscoveredProvider(discoveryId),
    )
  }

  async getConfig() {
    return this.providerPreferences.getConfig()
  }

  listPermissionRules() {
    return this.permissionRules.list()
  }

  addPermissionRule(input) {
    return this.permissionRules.add(input)
  }

  removePermissionRule(id) {
    return this.permissionRules.remove(id)
  }

  async setFallbackModels(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setFallbackModels(input),
    )
  }

  async setSubagentModelTiers(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setSubagentModelTiers(input),
    )
  }

  async setAuxiliaryModel(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setAuxiliaryModel(input),
    )
  }

  // 辅助模型解析失败（读配置出错等）一律当作未设置，后台杂活退回原来的模型。
  async resolveAuxiliaryModel() {
    return this.providerPreferences.resolveAuxiliaryModel().catch(() => null)
  }

  // 列出子代理定义及坏文件原因：不带会话或项目时只列全局定义（设置页）；
  // 带上时再叠加该项目的定义（同名覆盖全局），目录校验与新建会话一致，不接受任意路径。
  async listAgentProfiles({ sessionId = '', cwd = '' } = {}) {
    const directory = sessionId
      ? await this.sessionWorkspaceCwd(sessionId)
      : cwd
        ? await resolveWorkspaceDirectory(cwd, this.cwd)
        : ''
    return loadAgentProfiles({ agentDir: this.dataDir, cwd: directory })
  }

  async exportProviderConfig() {
    return this.providerPreferences.exportProviderConfig()
  }

  async importProviderConfig(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.importProviderConfig(input),
    )
  }

  async saveConfig(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.saveConfig(input, toolsFromConfig, TOOL_PRESETS),
    )
  }

  async setProviderConnection(id, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderConnection(id, input),
    )
  }

  async setProviderApiKey(id, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderApiKey(id, input),
    )
  }

  async setProviderEnabled(id, enabled) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderEnabled(id, enabled),
    )
  }

  async cloneProvider(id, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.cloneProvider(id, input),
    )
  }

  async createProvider(input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.createProvider(input),
    )
  }

  async setProviderModelOptions(providerId, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.setProviderModelOptions(providerId, input),
    )
  }

  async addProviderModel(providerId, input) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.addProviderModels(providerId, [input], { skipExisting: false }),
    )
  }

  async deleteProviderModel(providerId, modelId) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.deleteProviderModel(providerId, modelId),
    )
  }

  async reconcileDefaultModel() {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.reconcileDefaultModel(),
    )
  }

  async refreshProviderModels() {
    return this.providerPreferences.refreshProviderModels()
  }

  async discoverProviderModels(providerId, input = {}) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.discoverProviderModels(providerId, input),
    )
  }

  async discoverConnectionModels(input = {}) {
    return this.providerPreferences.discoverConnectionModels(input)
  }

  async addProviderModels(providerId, inputs, options = {}) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.addProviderModels(providerId, inputs, options),
    )
  }

  async deleteProvider(id) {
    return this.providerPreferences.withConfigurationWrite(() =>
      this.providerPreferences.deleteProvider(id),
    )
  }

  // 视觉模型状态：返回当前选中的模型和全部候选，供配置页切换视觉模型。
  async getVisualModelStatus() {
    const [image, video] = await Promise.all([
      this.visualGeneration.getModelStatus('image'),
      this.visualGeneration.getModelStatus('video'),
    ])
    return {
      image: image.model,
      video: video.model,
      imageModels: image.models,
      videoModels: video.models,
      imageSelection: image.selection,
      videoSelection: video.selection,
    }
  }

  async setVisualModel(kind, requestedModel) {
    return this.visualGeneration.setPreferredModel(kind, requestedModel)
  }

  // 视觉生成冒烟测试：用最小参数真实生成一张图，验证密钥与模型链路可用。
  // 产物写到数据目录（不污染用户工作区），并附带 base64 预览（超大文件跳过预览）。
  async testVisualGeneration() {
    const cwd = join(this.dataDir, 'visual-test')
    const result = await this.visualGeneration.generate({
      kind: 'image',
      prompt:
        'a small friendly robot mascot waving, flat vector illustration, soft pastel colors, plain background',
      outputName: 'config-test',
      cwd,
    })
    let previewDataUrl = ''
    try {
      const buffer = await readFile(result.path)
      if (buffer.length <= 6 * 1024 * 1024) {
        previewDataUrl = `data:${result.mimeType};base64,${buffer.toString('base64')}`
      }
    } catch {
      // 预览读取失败不影响测试结论：文件已生成即视为成功。
    }
    return { ...result, previewDataUrl }
  }
}
