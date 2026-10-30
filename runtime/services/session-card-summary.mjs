// 会话卡片摘要：看板卡片（GET /api/sessions?view=board 与 /api/board/events 的 card）的数据来源。
// 摘要只由 agent-event-bridge 投出的运行事件增量维护，常驻内存、按会话数封顶；
// 运行结束时把一份精简摘要写进会话元数据（pisper-sessions.json 的 boardCard），Runtime 重启后
// 已落盘的会话直接用它，绝不为看板重新扫描会话历史或构造消息分页。
// 所有输出文本都按码点截断并去掉控制字符 / ANSI 序列，保证 JSON.parse 与 serde_json 都能解析。
import { redactSecretText } from '../security/secret-redaction.mjs'
import {
  BOARD_CARD_OUTCOMES,
  approvalSeverity,
  deriveBoardStatus,
} from '../../shared/session-board-status.mjs'

/**
 * @typedef {import('../../shared/session-board-status.mjs').BoardDoneWindow} BoardDoneWindow
 * @typedef {import('../../shared/session-board-status.mjs').BoardCardOutcome} BoardCardOutcome
 * @typedef {{ type: 'model', stage: string } | { type: 'tool', name: string, argsSummary: string, startedAt: string | null }} CardActivity
 * @typedef {{ message: string, kind: string }} CardError
 * @typedef {{ added: number | null, removed: number | null, files: number | null, status: string }} CardChangeStats
 * @typedef {{ id: string, toolName: string, argsPreview: string, argsTruncated?: boolean, risk: string, severity: 'standard' | 'critical', reason: string, createdAt: string }} CardApproval
 * @typedef {{
 *   boardStatus: 'attention' | 'running' | 'queued' | 'done' | 'idle',
 *   runId?: string,
 *   activity?: CardActivity,
 *   startedAt?: string,
 *   lastActivityAt?: string,
 *   finishedAt?: string,
 *   outcome?: BoardCardOutcome,
 *   error?: CardError,
 *   approvals?: CardApproval[],
 *   queuedInputCount?: number,
 *   contextPct?: number,
 *   changeStats?: CardChangeStats,
 *   lastAssistantPreview?: string,
 *   tail?: string[],
 *   agents?: { running: number, total: number },
 *   usage?: { totalTokens: number },
 *   question?: Record<string, unknown>,
 * }} SessionCard
 * @typedef {{
 *   v: 1,
 *   outcome: BoardCardOutcome,
 *   finishedAt: string,
 *   error?: CardError,
 *   lastAssistantPreview?: string,
 *   changeStats?: CardChangeStats,
 *   contextPct?: number,
 *   usage?: { totalTokens: number },
 *   tail?: string[],
 * }} PersistedCard
 * @typedef {{ name: string, argsSummary: string, startedAt: string | null }} RunningTool
 * @typedef {{
 *   runId: string,
 *   startedAt: string,
 *   lastActivityAt: string,
 *   finishedAt: string,
 *   outcome: BoardCardOutcome | '',
 *   error: CardError | null,
 *   activity: CardActivity | null,
 *   runningTools: Map<string, RunningTool>,
 *   tailBuffer: string,
 *   tail: string[] | null,
 *   lastAssistantPreview: string,
 *   contextPct: number | null,
 *   totalTokens: number | null,
 *   changeStats: CardChangeStats | null,
 *   wroteFiles: boolean,
 *   changeTimer: ReturnType<typeof setTimeout> | null,
 *   changeLoading: boolean,
 *   changeAgain: boolean,
 * }} CardEntry
 */

// 字段上限（按码点计）：与规格 8.1 的 card 结构一一对应。
export const CARD_LIMITS = Object.freeze({
  argsSummary: 160,
  errorMessage: 240,
  argsPreview: 512,
  reason: 160,
  preview: 200,
  tailLines: 3,
  tailBytes: 512,
  tailLineChars: 200,
  toolName: 120,
  label: 40,
  approvals: 8,
})
// 尾巴滚动缓冲：只保留最近 4096 个码元，计算尾巴时再按行与字节裁剪。
const TAIL_BUFFER_CHARS = 4096
// 内存中最多保留的会话摘要数；超出时优先淘汰已结束的会话（落盘摘要仍在元数据里）。
export const MAX_CARD_ENTRIES = 256
// 写入类工具结束后合并 750ms 再读改动统计，避免连续 edit 时反复读快照索引。
export const CHANGE_STATS_DELAY_MS = 750
const WRITE_TOOLS = new Set(['edit', 'write'])
const RUN_ID_PATTERN = /^run_[A-Za-z0-9_-]{1,64}$/
const SENSITIVE_KEY_PATTERN =
  /api.?key|password|passwd|secret|token|authorization|cookie|credential/i
const ANSI_PATTERN =
  // eslint-disable-next-line no-control-regex
  /\u001b(?:\[[0-?]*[ -/]*[@-~]|\][^\u0007\u001b]*(?:\u0007|\u001b\\)?|[@-Z\\-_])/g
// eslint-disable-next-line no-control-regex
const CONTROL_PATTERN = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g
// eslint-disable-next-line no-control-regex
const LINE_CONTROL_PATTERN = /[\u0000-\u001f\u007f-\u009f]/g

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : {}
}

/** @param {unknown} value */
function text(value) {
  return typeof value === 'string' ? value : ''
}

/** @param {unknown} value @returns {number | null} */
function finiteNumber(value) {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** @param {unknown} value @returns {string} */
function isoTime(value) {
  if (typeof value !== 'string' || !value) return ''
  const time = Date.parse(value)
  return Number.isFinite(time) ? new Date(time).toISOString() : ''
}

/** 去掉终端 ANSI 转义序列（颜色、光标移动、OSC 标题等）。 @param {string} value */
export function stripAnsi(value) {
  return value.replace(ANSI_PATTERN, '')
}

/**
 * 按码点截断：超出上限时保留前 max-1 个码点并补“…”，不会把代理对切成两半。
 * @param {string} value @param {number} max
 */
export function clipCodePoints(value, max) {
  // 码元数不超过上限时码点数必然也不超过，省掉长文本的逐码点展开。
  if (value.length <= max) return value
  const points = Array.from(value.slice(0, max * 2 + 2))
  if (points.length <= max) return points.join('')
  return `${points.slice(0, Math.max(0, max - 1)).join('')}…`
}

/**
 * 按 UTF-8 字节截断（同样只在码点边界上切）。
 * @param {string} value @param {number} maxBytes
 */
export function clipUtf8Bytes(value, maxBytes) {
  if (Buffer.byteLength(value) <= maxBytes) return value
  const budget = maxBytes - Buffer.byteLength('…')
  let used = 0
  let result = ''
  for (const point of value) {
    const size = Buffer.byteLength(point)
    if (used + size > budget) break
    used += size
    result += point
  }
  return `${result}…`
}

/** 单行文本：去 ANSI、控制字符换成空格、压缩空白后按码点截断。 @param {unknown} value @param {number} max */
export function singleLine(value, max) {
  const line = stripAnsi(text(value)).replace(LINE_CONTROL_PATTERN, ' ').replace(/\s+/g, ' ').trim()
  return clipCodePoints(line, max)
}

/** 第一条非空行（最后一条回复的结果摘要）。 @param {unknown} value @param {number} max */
export function firstLine(value, max) {
  const source = text(value)
  // 只看开头一段：首行之后的长文本对摘要没有意义，也不必整段处理。
  for (const line of source.slice(0, max * 8 + 512).split('\n')) {
    const cleaned = singleLine(line, max)
    if (cleaned) return cleaned
  }
  return ''
}

/**
 * 尾巴：取最后 ≤3 个非空行，每行按码点截断、去 ANSI 与控制字符并脱敏，总量 ≤512 字节。
 * 进度条用的 \r 覆盖写只保留最后一段，与终端里看到的一致。
 * @param {unknown} value
 * @returns {string[]}
 */
export function tailLines(value) {
  const lines = []
  for (const raw of stripAnsi(text(value)).split('\n')) {
    const segments = raw.split('\r')
    let visible = ''
    for (let index = segments.length - 1; index >= 0 && !visible; index -= 1)
      if (segments[index].trim()) visible = segments[index]
    const line = clipCodePoints(
      redactSecretText(visible.replace(LINE_CONTROL_PATTERN, ' ').replace(/\s+$/u, '')),
      CARD_LIMITS.tailLineChars,
    )
    if (line.trim()) lines.push(line)
  }
  const kept = []
  let bytes = 0
  for (const line of lines.slice(-CARD_LIMITS.tailLines).reverse()) {
    const size = Buffer.byteLength(line) + (kept.length ? 1 : 0)
    if (bytes + size <= CARD_LIMITS.tailBytes) {
      kept.unshift(line)
      bytes += size
      continue
    }
    // 最新一行本身就超出字节上限时按字节截断保留；更早的行直接放弃。
    if (!kept.length) kept.unshift(clipUtf8Bytes(line, CARD_LIMITS.tailBytes))
    break
  }
  return kept
}

/**
 * 工具参数的一行摘要（≤160 码点）：bash 取命令，文件类取路径，其余取前几个简单参数。
 * 敏感键直接跳过，命令文本再走一遍密钥脱敏。
 * @param {string} toolName @param {unknown} args
 */
export function toolArgsSummary(toolName, args) {
  const value = record(args)
  const direct =
    text(value.command) ||
    text(value.path) ||
    text(value.file_path) ||
    text(value.url) ||
    text(value.query) ||
    text(value.pattern)
  if (direct) return singleLine(redactSecretText(direct), CARD_LIMITS.argsSummary)
  const parts = []
  for (const [key, child] of Object.entries(value)) {
    if (parts.length >= 3) break
    if (SENSITIVE_KEY_PATTERN.test(key)) continue
    if (typeof child === 'string' || typeof child === 'number' || typeof child === 'boolean')
      parts.push(`${key}=${String(child)}`)
  }
  return singleLine(redactSecretText(parts.join(' ')), CARD_LIMITS.argsSummary) || toolName
}

/**
 * 审批参数预览（≤512 码点）：bash 是完整命令，其它工具逐行“键: 值”，与前端确认层的展示格式一致。
 * 输入已经过权限服务的 safeArgs（敏感键隐藏、超长内容省略）；这里只去 ANSI / 控制字符（保留换行和制表符）。
 * 截断时 argsTruncated 为 true，批准前完整参数必须从 /live 读取。
 * @param {string} toolName @param {unknown} args
 */
export function approvalArgsPreview(toolName, args) {
  const value = record(args)
  const command = text(value.command)
  const raw =
    toolName === 'bash' && command
      ? command
      : Object.entries(value)
          .map(([key, child]) => `${key}: ${typeof child === 'string' ? child : safeJson(child)}`)
          .join('\n')
  const cleaned = stripAnsi(raw).replace(CONTROL_PATTERN, ' ')
  const clipped = clipCodePoints(cleaned, CARD_LIMITS.argsPreview)
  return { text: clipped, truncated: clipped !== cleaned }
}

/** @param {unknown} value */
function safeJson(value) {
  try {
    return JSON.stringify(value) ?? ''
  } catch {
    return ''
  }
}

/** @param {unknown} value @returns {CardChangeStats | null} */
function normalizeChangeStats(value) {
  const stats = record(value)
  const status = text(stats.status)
  if (!['known', 'partial', 'unavailable'].includes(status)) return null
  /** @param {unknown} input */
  const count = (input) => {
    const number = finiteNumber(input)
    return number === null ? null : Math.max(0, Math.round(number))
  }
  return {
    added: count(stats.added),
    removed: count(stats.removed),
    files: count(stats.files ?? stats.changedFiles),
    status,
  }
}

/** @param {unknown} value @returns {number | null} */
function normalizePercent(value) {
  const number = finiteNumber(value)
  return number === null ? null : Math.min(100, Math.max(0, Math.round(number * 10) / 10))
}

/** @param {unknown} value @returns {CardError | null} */
function normalizeError(value) {
  const error = record(value)
  // 错误文本（例如 Provider 回显的 401 正文）会经事件流广播、失败时还会落盘：与尾巴同样先脱敏。
  // 读取旧的落盘摘要也走这里，旧数据同样被覆盖到。
  const message = singleLine(
    typeof error.message === 'string'
      ? redactSecretText(error.message.slice(0, CARD_LIMITS.errorMessage * 16))
      : error.message,
    CARD_LIMITS.errorMessage,
  )
  if (!message) return null
  return { message, kind: singleLine(error.kind, CARD_LIMITS.label) || 'runtime' }
}

/** @param {unknown} value @returns {BoardCardOutcome | ''} */
function normalizeOutcome(value) {
  return typeof value === 'string' &&
    /** @type {readonly string[]} */ (BOARD_CARD_OUTCOMES).includes(value)
    ? /** @type {BoardCardOutcome} */ (value)
    : ''
}

/**
 * 读取落盘摘要：pisper-sessions.json 可能被手工改动或来自别的版本，逐字段校验，坏数据当作没有。
 * @param {unknown} value
 * @returns {PersistedCard | null}
 */
export function normalizePersistedCard(value) {
  const stored = record(value)
  const outcome = normalizeOutcome(stored.outcome)
  const finishedAt = isoTime(stored.finishedAt)
  if (!outcome || !finishedAt) return null
  /** @type {PersistedCard} */
  const card = { v: 1, outcome, finishedAt }
  const error = normalizeError(stored.error)
  if (error && outcome === 'failed') card.error = error
  const preview = firstLine(stored.lastAssistantPreview, CARD_LIMITS.preview)
  if (preview) card.lastAssistantPreview = preview
  const changeStats = normalizeChangeStats(stored.changeStats)
  if (changeStats) card.changeStats = changeStats
  const contextPct = normalizePercent(stored.contextPct)
  if (contextPct !== null) card.contextPct = contextPct
  const totalTokens = finiteNumber(record(stored.usage).totalTokens)
  if (totalTokens !== null && totalTokens >= 0)
    card.usage = { totalTokens: Math.round(totalTokens) }
  if (outcome === 'failed' && Array.isArray(stored.tail)) {
    const tail = tailLines(stored.tail.filter((line) => typeof line === 'string').join('\n'))
    if (tail.length) card.tail = tail
  }
  return card
}

/** @returns {CardEntry} */
function emptyEntry() {
  return {
    runId: '',
    startedAt: '',
    lastActivityAt: '',
    finishedAt: '',
    outcome: '',
    error: null,
    activity: null,
    runningTools: new Map(),
    tailBuffer: '',
    tail: [],
    lastAssistantPreview: '',
    contextPct: null,
    totalTokens: null,
    changeStats: null,
    wroteFiles: false,
    changeTimer: null,
    changeLoading: false,
    changeAgain: false,
  }
}

/** @param {CardEntry} entry */
function latestRunningTool(entry) {
  let latest = null
  for (const tool of entry.runningTools.values()) latest = tool
  return latest
}

/** @param {CardEntry} entry @param {string} stage */
function setModelStage(entry, stage) {
  // 并行工具仍在执行时，活动脚注继续显示工具，不被思考 / 回复事件覆盖。
  if (entry.runningTools.size) return
  entry.activity = { type: 'model', stage: singleLine(stage, CARD_LIMITS.label) || 'thinking' }
}

/** @param {CardEntry} entry @param {string} output */
function replaceTail(entry, output) {
  entry.tailBuffer = output.length > TAIL_BUFFER_CHARS ? output.slice(-TAIL_BUFFER_CHARS) : output
  entry.tail = null
}

/** @param {CardEntry} entry */
function currentTail(entry) {
  if (entry.tail === null) entry.tail = tailLines(entry.tailBuffer)
  return entry.tail
}

export class SessionCardSummaries {
  /**
   * @param {{
   *   loadChangeSummary?: (sessionId: string) => Promise<unknown>,
   *   persist?: (sessionId: string, card: PersistedCard, options: { save: boolean }) => void,
   *   onChange?: (sessionId: string) => void,
   *   maxEntries?: number,
   *   changeStatsDelayMs?: number,
   *   now?: () => number,
   * }} [options]
   */
  constructor({
    loadChangeSummary,
    persist,
    onChange,
    maxEntries = MAX_CARD_ENTRIES,
    changeStatsDelayMs = CHANGE_STATS_DELAY_MS,
    now = Date.now,
  } = {}) {
    this.loadChangeSummary = loadChangeSummary || null
    this.persist = persist || null
    this.onChange = onChange || null
    this.maxEntries = Math.max(1, Math.floor(maxEntries))
    this.changeStatsDelayMs = Math.max(0, changeStatsDelayMs)
    this.now = now
    /** @type {Map<string, CardEntry>} */
    this.entries = new Map()
    // 已删除会话的墓碑：运行途中删除时，本轮收尾的事件不会把摘要条目（以及落盘的 boardCard）
    // 重新建出来。会话 id 不复用，按插入顺序只留最近的一批。
    /** @type {Set<string>} */
    this.deleted = new Set()
    this.disposed = false
  }

  /** @param {string} sessionId */
  isDeleted(sessionId) {
    return this.deleted.has(sessionId)
  }

  /** @param {string} sessionId */
  entry(sessionId) {
    return this.entries.get(sessionId) || null
  }

  /** 取出（必要时新建）并移到 LRU 末尾。 @param {string} sessionId */
  touch(sessionId) {
    // 已删除的会话只拿到一个不入表的临时条目：调用方照常写，但不会留下内存或落盘残留。
    if (this.deleted.has(sessionId)) return emptyEntry()
    let entry = this.entries.get(sessionId)
    if (entry) this.entries.delete(sessionId)
    else entry = emptyEntry()
    this.entries.set(sessionId, entry)
    this.evict()
    return entry
  }

  evict() {
    while (this.entries.size > this.maxEntries) {
      // 优先淘汰已结束的会话；全部仍在运行时退而淘汰最旧的一条。
      let victim = null
      for (const [id, entry] of this.entries) {
        if (!entry.runningTools.size && entry.finishedAt) {
          victim = id
          break
        }
      }
      victim ??= this.entries.keys().next().value ?? null
      if (victim === null) break
      this.forget(victim)
    }
  }

  /** @param {string} sessionId @param {{ deleted?: boolean }} [options] */
  forget(sessionId, { deleted = false } = {}) {
    const entry = this.entries.get(sessionId)
    if (entry?.changeTimer) clearTimeout(entry.changeTimer)
    this.entries.delete(sessionId)
    if (!deleted || !sessionId) return
    this.deleted.add(sessionId)
    while (this.deleted.size > this.maxEntries) {
      const oldest = this.deleted.values().next().value
      if (oldest === undefined) break
      this.deleted.delete(oldest)
    }
  }

  /**
   * 新一轮运行开始：清掉上一轮的结局、错误、尾巴与工具，保留会话级的回复摘要、改动统计与用量。
   * @param {string} sessionId @param {{ runId?: unknown, startedAt?: unknown }} [run]
   */
  beginRun(sessionId, { runId, startedAt } = {}) {
    if (!sessionId || this.disposed) return
    const entry = this.touch(sessionId)
    const at = isoTime(startedAt) || new Date(this.now()).toISOString()
    entry.runId = typeof runId === 'string' && RUN_ID_PATTERN.test(runId) ? runId : ''
    entry.startedAt = at
    entry.lastActivityAt = at
    entry.finishedAt = ''
    entry.outcome = ''
    entry.error = null
    entry.activity = { type: 'model', stage: 'starting' }
    entry.runningTools.clear()
    entry.tailBuffer = ''
    entry.tail = []
    entry.wroteFiles = false
  }

  /**
   * 增量维护：参数就是 agent-runtime 发给客户端的同一帧事件（事件名 + 数据），只读不改。
   * 审批、子代理、排队等状态在投影时从各自的服务读取，这里只记最近活动时间。
   * @param {string} sessionId @param {string} event @param {unknown} payload
   */
  observe(sessionId, event, payload) {
    if (!sessionId || this.disposed) return
    const entry = this.entries.get(sessionId)
    // 没有 beginRun 的会话（例如侧聊或测试替身直接发事件）不建摘要，避免为任意事件占内存。
    if (!entry) return
    const data = record(payload)
    entry.lastActivityAt = new Date(this.now()).toISOString()
    switch (event) {
      case 'text_delta':
      case 'text_patch':
        setModelStage(entry, 'responding')
        break
      case 'text_end': {
        const preview = firstLine(data.text, CARD_LIMITS.preview)
        if (preview) entry.lastAssistantPreview = preview
        break
      }
      case 'thinking_patch':
      case 'thinking_reset':
        setModelStage(entry, 'thinking')
        break
      case 'agent_lifecycle': {
        const activity = record(data.currentActivity)
        if (activity.type === 'model' && typeof activity.stage === 'string')
          setModelStage(entry, activity.stage)
        break
      }
      case 'compaction_start':
        setModelStage(entry, 'compacting')
        break
      case 'compaction_end':
        setModelStage(entry, 'thinking')
        break
      case 'retry':
        setModelStage(entry, 'waiting_retry')
        break
      case 'tool_start': {
        const id = text(data.id)
        const name = singleLine(data.name, CARD_LIMITS.toolName) || 'tool'
        const tool = {
          name,
          argsSummary: toolArgsSummary(name, data.args),
          startedAt: isoTime(data.startedAt) || entry.lastActivityAt,
        }
        if (id) entry.runningTools.set(id, tool)
        // 并行工具数有限，这里再兜底防止异常事件让映射无限增长。
        while (entry.runningTools.size > 16) {
          const oldest = entry.runningTools.keys().next().value
          if (oldest === undefined) break
          entry.runningTools.delete(oldest)
        }
        entry.activity = { type: 'tool', ...tool }
        replaceTail(entry, '')
        break
      }
      case 'tool_update': {
        if (typeof data.output === 'string') replaceTail(entry, data.output)
        else if (typeof data.message === 'string' && data.message) replaceTail(entry, data.message)
        break
      }
      case 'tool_end': {
        const id = text(data.id)
        const name = text(data.name)
        if (id) entry.runningTools.delete(id)
        if (typeof data.output === 'string' && data.output) replaceTail(entry, data.output)
        else if (data.error === true && typeof data.message === 'string' && data.message)
          replaceTail(entry, data.message)
        const next = latestRunningTool(entry)
        entry.activity = next
          ? { type: 'tool', ...next }
          : { type: 'model', stage: 'processing_result' }
        if (WRITE_TOOLS.has(name) && data.error !== true) {
          entry.wroteFiles = true
          this.scheduleChangeStats(sessionId, this.changeStatsDelayMs)
        }
        break
      }
      case 'context_usage': {
        const percent = normalizePercent(data.percent)
        if (percent !== null) entry.contextPct = percent
        break
      }
      case 'session_usage': {
        const total = finiteNumber(data.totalTokens)
        if (total !== null && total >= 0) entry.totalTokens = Math.round(total)
        break
      }
      default:
        break
    }
  }

  /**
   * 运行结束：记录结局与错误，并返回要写进会话元数据的精简摘要（调用方负责落盘）。
   * @param {string} sessionId
   * @param {{
   *   outcome: BoardCardOutcome,
   *   finishedAt?: unknown,
   *   error?: { message?: unknown, kind?: unknown } | null,
   *   text?: unknown,
   *   contextPercent?: unknown,
   *   totalTokens?: unknown,
   * }} result
   * @returns {PersistedCard | null}
   */
  finishRun(
    sessionId,
    { outcome, finishedAt, error, text: finalText, contextPercent, totalTokens },
  ) {
    if (!sessionId || this.disposed || this.deleted.has(sessionId)) return null
    const entry = this.touch(sessionId)
    const at = isoTime(finishedAt) || new Date(this.now()).toISOString()
    entry.finishedAt = at
    entry.lastActivityAt = at
    entry.outcome = normalizeOutcome(outcome) || 'completed'
    entry.error = entry.outcome === 'failed' ? normalizeError(error) : null
    entry.activity = null
    entry.runningTools.clear()
    const preview = firstLine(finalText, CARD_LIMITS.preview)
    if (preview) entry.lastAssistantPreview = preview
    const percent = normalizePercent(contextPercent)
    if (percent !== null) entry.contextPct = percent
    const total = finiteNumber(totalTokens)
    if (total !== null && total >= 0) entry.totalTokens = Math.round(total)
    // 本轮有写入时收尾再读一次改动统计：最后一次写入可能还在 750ms 合并窗口里。
    if (entry.wroteFiles) this.scheduleChangeStats(sessionId, 0)
    return this.persisted(sessionId)
  }

  /** @param {string} sessionId @returns {PersistedCard | null} */
  persisted(sessionId) {
    const entry = this.entries.get(sessionId)
    if (!entry?.finishedAt || !entry.outcome) return null
    /** @type {PersistedCard} */
    const card = { v: 1, outcome: entry.outcome, finishedAt: entry.finishedAt }
    if (entry.error && entry.outcome === 'failed') card.error = { ...entry.error }
    if (entry.lastAssistantPreview) card.lastAssistantPreview = entry.lastAssistantPreview
    if (entry.changeStats) card.changeStats = { ...entry.changeStats }
    if (entry.contextPct !== null) card.contextPct = entry.contextPct
    if (entry.totalTokens !== null) card.usage = { totalTokens: entry.totalTokens }
    if (entry.outcome === 'failed') {
      const tail = currentTail(entry)
      if (tail.length) card.tail = [...tail]
    }
    return card
  }

  /** @param {string} sessionId @param {number} delay */
  scheduleChangeStats(sessionId, delay) {
    const entry = this.entries.get(sessionId)
    if (!entry || !this.loadChangeSummary || this.disposed) return
    if (entry.changeTimer) clearTimeout(entry.changeTimer)
    entry.changeTimer = setTimeout(() => {
      entry.changeTimer = null
      void this.refreshChangeStats(sessionId)
    }, delay)
    entry.changeTimer.unref?.()
  }

  /** @param {string} sessionId */
  async refreshChangeStats(sessionId) {
    const entry = this.entries.get(sessionId)
    if (!entry || !this.loadChangeSummary || this.disposed) return
    // 同一会话同时只读一次；读取期间又有写入时，结束后再补读一次。
    if (entry.changeLoading) {
      entry.changeAgain = true
      return
    }
    entry.changeLoading = true
    try {
      const stats = normalizeChangeStats(await this.loadChangeSummary(sessionId))
      if (stats && this.entries.get(sessionId) === entry && !this.disposed) {
        entry.changeStats = stats
        this.onChange?.(sessionId)
        // 运行已结束时摘要已经落过盘，这里补写改动统计。
        const persisted = entry.finishedAt ? this.persisted(sessionId) : null
        if (persisted) this.persist?.(sessionId, persisted, { save: true })
      }
    } catch {
      // 改动统计只是卡片附加信息，读取失败时保留上一次的结果。
    } finally {
      entry.changeLoading = false
      if (entry.changeAgain) {
        entry.changeAgain = false
        this.scheduleChangeStats(sessionId, this.changeStatsDelayMs)
      }
    }
  }

  dispose() {
    this.disposed = true
    for (const entry of this.entries.values()) {
      if (entry.changeTimer) clearTimeout(entry.changeTimer)
    }
    this.entries.clear()
  }
}

/**
 * @param {unknown} approval
 * @returns {CardApproval | null}
 */
function projectApproval(approval) {
  const value = record(approval)
  const id = text(value.id)
  if (!id) return null
  const toolName = singleLine(value.toolName, CARD_LIMITS.toolName)
  const preview = approvalArgsPreview(toolName, value.args)
  return {
    id,
    toolName,
    argsPreview: preview.text,
    ...(preview.truncated ? { argsTruncated: true } : {}),
    risk: singleLine(value.risk, CARD_LIMITS.label),
    severity: approvalSeverity(value),
    reason: singleLine(value.reason, CARD_LIMITS.reason),
    createdAt: isoTime(value.createdAt),
  }
}

/** @param {unknown} activity @returns {CardActivity | null} */
function liveActivity(activity) {
  const value = record(activity)
  if (value.type === 'tool' && value.status === 'running') {
    const name = singleLine(value.name, CARD_LIMITS.toolName) || 'tool'
    return {
      type: 'tool',
      name,
      argsSummary: toolArgsSummary(name, value.args),
      startedAt: isoTime(value.startedAt) || null,
    }
  }
  if (value.type === 'model' && typeof value.stage === 'string')
    return { type: 'model', stage: singleLine(value.stage, CARD_LIMITS.label) }
  if (value.type === 'compaction') return { type: 'model', stage: 'compacting' }
  return null
}

/**
 * 投影一张卡片：事件摘要（内存）> 落盘摘要（元数据）；审批、子代理、排队数在投影时从权威服务读取。
 * boardStatus 与前端降级推导走同一个 deriveBoardStatus，idle 表示不上看板。
 * @param {{
 *   session: Record<string, unknown>,
 *   entry?: CardEntry | null,
 *   persisted?: PersistedCard | null,
 *   live?: Record<string, unknown> | null,
 *   approvals?: readonly unknown[],
 *   agents?: readonly unknown[],
 *   question?: Record<string, unknown> | null,
 *   capabilities?: import('../../shared/session-board-status.mjs').BoardCapabilities,
 *   now?: number,
 *   doneWindow?: BoardDoneWindow,
 *   timeZoneOffsetMinutes?: number,
 * }} input
 * @returns {SessionCard}
 */
export function projectSessionCard({
  session,
  entry = null,
  persisted = null,
  live = null,
  approvals = [],
  agents = [],
  question = null,
  capabilities = {},
  now = Date.now(),
  doneWindow = '7d',
  timeZoneOffsetMinutes,
}) {
  const streaming = session.streaming === true
  const liveState = record(live)
  /** @type {Omit<SessionCard, 'boardStatus'>} */
  const card = {}
  const runId = entry?.runId || text(liveState.runId)
  if (runId && RUN_ID_PATTERN.test(runId)) card.runId = runId
  if (streaming) {
    const activity = entry?.activity || liveActivity(liveState.currentActivity)
    if (activity) card.activity = activity
  }
  const startedAt = entry?.startedAt || isoTime(liveState.startedAt)
  if (startedAt) card.startedAt = startedAt
  const lastActivityAt = entry?.lastActivityAt || isoTime(liveState.lastActivityAt)
  if (lastActivityAt) card.lastActivityAt = lastActivityAt
  if (!streaming) {
    // 本进程里跑过的会话以内存摘要为准；还没结束（被强制回收等）时退回落盘摘要。
    const finished = entry?.finishedAt ? entry : null
    const finishedAt = finished?.finishedAt || persisted?.finishedAt || ''
    const outcome = finished?.outcome || persisted?.outcome || ''
    if (finishedAt) card.finishedAt = finishedAt
    if (outcome) card.outcome = outcome
    const error = finished ? finished.error : persisted?.error
    if (outcome === 'failed' && error) card.error = { ...error }
  }
  const projected = approvals
    .map(projectApproval)
    .filter((approval) => approval !== null)
    .sort((left, right) => left.createdAt.localeCompare(right.createdAt))
    .slice(0, CARD_LIMITS.approvals)
  if (projected.length) card.approvals = projected
  const queued = Array.isArray(liveState.queuedInputs) ? liveState.queuedInputs.length : 0
  if (streaming && queued > 0) card.queuedInputCount = queued
  const contextPct = entry?.contextPct ?? persisted?.contextPct ?? null
  if (contextPct !== null) card.contextPct = contextPct
  const changeStats = entry?.changeStats || persisted?.changeStats || null
  if (changeStats) card.changeStats = { ...changeStats }
  const preview = entry?.lastAssistantPreview || persisted?.lastAssistantPreview || ''
  if (preview) card.lastAssistantPreview = preview
  // 尾巴只在运行中与失败后展示（失败的那一行帮助判断）；其它状态不带，卡片保持紧凑。
  const tail = entry ? currentTail(entry) : persisted?.tail || []
  if (tail.length && (streaming || card.outcome === 'failed')) card.tail = [...tail]
  if (agents.length) {
    const running = agents.filter((agent) =>
      ['queued', 'starting', 'running'].includes(text(record(agent).status)),
    ).length
    card.agents = { running, total: agents.length }
  }
  const totalTokens = entry?.totalTokens ?? persisted?.usage?.totalTokens ?? null
  if (totalTokens !== null) card.usage = { totalTokens }
  // 等你回答（ask_user）：问题摘要已在提问服务里归一化、限长，原样带上。
  if (question) card.question = question
  const derived = deriveBoardStatus(
    {
      ...session,
      card: {
        approvals: projected,
        startedAt: card.startedAt,
        outcome: card.outcome,
        ...(card.question ? { question: card.question } : {}),
      },
    },
    { now, doneWindow, timeZoneOffsetMinutes, capabilities },
  )
  return { boardStatus: derived.lane ?? 'idle', ...card }
}
