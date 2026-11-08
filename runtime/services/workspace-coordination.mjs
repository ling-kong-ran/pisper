// 同一文件夹里多个会话之间的协作。父会话和它派出的子 Agent 由团队工作流分工；
// 互不相干的会话（包括 Bot）彼此不知道对方，靠这里的台账协调：
// - 写入前检查：每个 Agent 记下自己读到、写下的每个文件的内容哈希；write/edit 执行前对比磁盘，
//   读过之后被别人改过（隔壁会话、同会话的另一个 Agent、命令行、编辑器、git）就拒绝这次写入，
//   让它先重新读，不把别人的修改整个盖掉。只比内容不比工具调用，任何途径的改动都看得见。
// - 隔壁提醒：一个会话改了别的会话读过或改过的文件，给那个会话留一条提示，在它下一次调用工具时
//   附在结果末尾（不进系统提示词、不打断它，也不替它开一轮）。
// - 谁在这里改文件：会话页据此提示“另有会话最近在这个文件夹改文件”。
// - 会话之间的留言（session_message）：对方在跑就插进它的下一步；没在跑先存着，
//   等用户发下一条消息或点“现在处理”时一起交给它，从不自己唤醒对方（两边来回回话不会变成死循环）。
// 文件台账只在内存里：Runtime 重启后各会话重新读文件即可；留言落盘，重启不丢。
import { createHash, randomUUID } from 'node:crypto'
import { lstat, readFile, realpath } from 'node:fs/promises'
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { resolveToCwd } from '../runtime/pi-coding-agent.mjs'

export const ABSENT = 'absent'
// 太大或不是普通文件：不比较，照常放行。
export const UNHASHABLE = 'unhashable'
export const MAX_FINGERPRINT_BYTES = 16 * 1024 * 1024
// “最近在这里改文件”的时间窗。
export const NEIGHBOUR_WINDOW_MS = 30 * 60_000
export const MAX_SESSION_MESSAGE_CHARS = 4000
export const SESSION_MESSAGE_LIMIT = 20
export const SESSION_MESSAGE_WINDOW_MS = 60 * 60_000
const MAX_INBOX_MESSAGES = 20
const MAX_TRACKED_FILES = 5000
const MAX_VIEWS = 200
const MAX_VIEW_FILES = 2000
const MAX_NOTE_FILES = 50
const NOTE_TTL_MS = 24 * 60 * 60_000
// 同一 Agent 对同一文件的写入还没走完（嵌套调用并发时）：期间不拿半截状态比较。
const INFLIGHT_TTL_MS = 2 * 60_000
const NOTE_FILES_SHOWN = 8
const INBOX_VERSION = 1

const WRITE_TOOLS = new Set(['write', 'edit'])

// 工具调用 → 文件操作；call_tool 按它实际调用的工具算。
export function fileOperation(name, args) {
  if (name === 'call_tool') {
    name = String(args?.name || '').trim()
    args = args?.arguments
  }
  const path = typeof args?.path === 'string' ? args.path.trim() : ''
  if (!path) return null
  if (WRITE_TOOLS.has(name)) return { kind: 'write', tool: name, path }
  if (name === 'read') return { kind: 'read', tool: name, path }
  return null
}

function sha256(value) {
  return createHash('sha256').update(value).digest('hex')
}

function inside(root, path) {
  const child = relative(root, path)
  return !child || (!isAbsolute(child) && child !== '..' && !child.startsWith(`..${sep}`))
}

// 两个会话的目录有包含关系就算同一个项目。
export function foldersOverlap(left, right) {
  if (!left || !right) return false
  const a = resolve(left)
  const b = resolve(right)
  return inside(a, b) || inside(b, a)
}

// 文件身份：真实路径（符号链接、相对路径都归一到同一个键）；Windows 不分大小写。
export async function fileKey(file) {
  const absolute = resolve(file)
  let real = await realpath(absolute).catch(() => null)
  if (!real) {
    const parent = await realpath(dirname(absolute)).catch(() => null)
    real = parent ? join(parent, basename(absolute)) : absolute
  }
  return process.platform === 'win32' ? real.toLowerCase() : real
}

export async function fingerprint(file) {
  const info = await lstat(file).catch((error) => {
    if (error?.code === 'ENOENT' || error?.code === 'ENOTDIR') return null
    throw error
  })
  if (!info) return ABSENT
  if (!info.isFile() || info.size > MAX_FINGERPRINT_BYTES) return UNHASHABLE
  return sha256(await readFile(file))
}

function displayPath(cwd, file) {
  if (cwd && inside(resolve(cwd), file)) {
    const path = relative(resolve(cwd), file).replaceAll('\\', '/')
    if (path) return path
  }
  return file.replaceAll('\\', '/')
}

function ago(now, at) {
  const minutes = Math.max(0, Math.round((now - at) / 60_000))
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  return `${Math.round(minutes / 60)} h ago`
}

function quoted(title) {
  return `"${String(title || 'Untitled').replaceAll('"', "'")}"`
}

// 留言的来源标记：放在附件上下文里（转录不显示），转录投影据此标出“来自会话「X」”。
const SESSION_MESSAGE_TAG = /\[pisper:session-message from=([A-Za-z0-9%._~!*'()-]{3,4000})\]/

export function sessionMessageTag(senders) {
  const value = senders.map((sender) => ({
    id: String(sender.id || '').slice(0, 200),
    title: String(sender.title || '').slice(0, 80),
  }))
  return `[pisper:session-message from=${encodeURIComponent(JSON.stringify(value))}]`
}

export function parseSessionMessageTag(text) {
  const match = SESSION_MESSAGE_TAG.exec(String(text || ''))
  if (!match) return null
  try {
    const value = JSON.parse(decodeURIComponent(match[1]))
    if (!Array.isArray(value)) return null
    const senders = value
      .filter((item) => item && typeof item.id === 'string' && item.id)
      .slice(0, 10)
      .map((item) => ({ id: item.id, title: typeof item.title === 'string' ? item.title : '' }))
    return senders.length ? senders : null
  } catch {
    return null
  }
}

// 交给对方模型的留言：正文给人看，标记与说明放在附件上下文里（marker 由调用方给，避免循环依赖）。
export function sessionMessageContent(messages, attachmentMarker) {
  const senders = []
  for (const message of messages)
    if (!senders.some((sender) => sender.id === message.from.id)) senders.push(message.from)
  const body =
    messages.length === 1
      ? messages[0].text
      : messages.map((message) => `「${message.from.title}」：${message.text}`).join('\n\n')
  const names = senders.map((sender) => `「${sender.title}」`).join('、')
  const context = [
    sessionMessageTag(senders),
    `这条消息来自同一个文件夹里的会话${names}，不是用户：它在协调对文件的修改。按需要照做或回话；` +
      '回话用 session_message 发给它（工具列表里没有时经 call_tool 调用）。不需要回的不回，不发客套话。',
  ].join('\n')
  return `${body}${attachmentMarker}${context}`
}

export class WorkspaceCoordination {
  constructor({
    dataDir = '',
    now = () => Date.now(),
    describeSession = () => null,
    // (file) => 工作目录包含这个文件的会话 id（包括 Runtime 启动后还没跑过、不在台账里的会话）。
    sessionsNear = () => [],
    onChange = () => {},
    warn = () => {},
  } = {}) {
    this.inboxPath = dataDir ? join(dataDir, 'session-messages.json') : ''
    this.now = now
    // (sessionId) => { title, running } | null
    this.describeSession = describeSession
    this.sessionsNear = sessionsNear
    this.onChange = onChange
    this.warn = warn
    this.installed = new WeakSet()
    // 每个 Agent 看到的文件内容：viewKey（Pi 会话自己的 id）→ Map<文件键, 哈希>。
    // 用会话 id 而不是会话对象：运行时被回收重建后，同一个会话的模型仍记得它读过的内容。
    this.views = new Map()
    this.inflight = new Map()
    // 文件键 → { file, writer: { sessionId, viewKey, at, hash }, touchedBy: Map<sessionId, at> }
    this.files = new Map()
    // sessionId → { cwd, wrote: Map<文件键, { file, at }> }
    this.sessions = new Map()
    // sessionId → Map<文件键, { from, file, at }>：等着交给它的“隔壁改了你用过的文件”。
    this.notes = new Map()
    this.inbox = new Map()
    this.sent = new Map()
    this.loaded = null
    this.writing = Promise.resolve()
    this.revision = 0
  }

  title(sessionId) {
    return this.describeSession(sessionId)?.title || 'Untitled'
  }

  bump(sessionIds = []) {
    this.revision += 1
    try {
      this.onChange([...new Set(sessionIds)])
    } catch {
      // 通知只用来刷新界面，失败不影响台账。
    }
  }

  view(viewKey) {
    let view = this.views.get(viewKey)
    if (view) {
      // 最近用过的放到最后，淘汰时先丢最久没动的。
      this.views.delete(viewKey)
      this.views.set(viewKey, view)
      return view
    }
    view = new Map()
    this.views.set(viewKey, view)
    while (this.views.size > MAX_VIEWS) this.views.delete(this.views.keys().next().value)
    return view
  }

  remember(viewKey, key, hash) {
    const view = this.view(viewKey)
    view.delete(key)
    view.set(key, hash)
    while (view.size > MAX_VIEW_FILES) view.delete(view.keys().next().value)
  }

  fileRecord(key, file) {
    let record = this.files.get(key)
    if (!record) {
      record = { file, writer: null, touchedBy: new Map() }
      this.files.set(key, record)
      while (this.files.size > MAX_TRACKED_FILES) this.files.delete(this.files.keys().next().value)
    }
    record.file = file
    return record
  }

  registerSession(sessionId, cwd) {
    const entry = this.sessions.get(sessionId) || { cwd, wrote: new Map() }
    entry.cwd = cwd
    this.sessions.set(sessionId, entry)
    return entry
  }

  // 在 Agent 会话上挂钩子。须装在文件快照之前：被拒绝的写入不记快照。
  // viewKey 区分同一会话里的不同 Agent（子 Agent 的写入也归属父会话 sessionId）。
  install(session, { sessionId, cwd, viewKey = session?.sessionId || sessionId }) {
    if (!session?.agent || !sessionId || !cwd || this.installed.has(session)) return
    this.installed.add(session)
    this.registerSession(sessionId, cwd)
    const before = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      const decision = await before?.(context, signal)
      if (decision?.block || signal?.aborted) return decision
      const op = fileOperation(context.toolCall?.name, context.args)
      if (op?.kind !== 'write') return decision
      try {
        const reason = await this.checkWrite({ sessionId, viewKey, cwd, op })
        if (reason) return { block: true, reason }
      } catch (error) {
        // 检查本身失败（读不了文件等）不拦写入，写入工具自己会报真实错误。
        this.warn(error)
      }
      return decision
    }
    const after = session.agent.afterToolCall
    session.agent.afterToolCall = async (context, signal) => {
      const override = await after?.(context, signal)
      try {
        const op = fileOperation(context.toolCall?.name, context.args)
        if (op)
          await this.recordResult({
            sessionId,
            viewKey,
            cwd,
            op,
            failed: override?.isError ?? context.isError,
          })
        const note = this.takeNotes(sessionId, cwd)
        if (!note) return override
        const content = override?.content ?? context.result?.content ?? []
        return {
          ...(override || {}),
          content: [...content, { type: 'text', text: note }],
          structuredContent: override
            ? override.structuredContent
            : context.result?.structuredContent,
        }
      } catch (error) {
        // 这里抛错会把工具结果变成错误：台账出问题只记一笔，结果原样交回。
        this.warn(error)
        return override
      }
    }
  }

  inflightSet(viewKey) {
    let set = this.inflight.get(viewKey)
    if (!set) {
      set = new Map()
      this.inflight.set(viewKey, set)
    }
    return set
  }

  // 返回拒绝原因（给模型看），可以写时返回空字符串。
  async checkWrite({ sessionId, viewKey, cwd, op }) {
    const file = resolveToCwd(op.path, cwd)
    const key = await fileKey(file)
    const pending = this.inflightSet(viewKey)
    const startedAt = pending.get(key)
    if (startedAt && this.now() - startedAt < INFLIGHT_TTL_MS) return ''
    const current = await fingerprint(file)
    const seen = this.views.get(viewKey)?.get(key)
    const writer = this.files.get(key)?.writer
    const path = displayPath(cwd, file)
    let reason = ''
    // 文件已经不在了：写下去不会盖掉任何人的内容（edit 自己会报找不到）。
    if (current === ABSENT) {
      pending.set(key, this.now())
      return ''
    }
    if (seen) {
      if (seen !== current && seen !== UNHASHABLE && current !== UNHASHABLE)
        reason = `${path} has changed since you last read or wrote it (${this.changedBy({ sessionId, viewKey, writer, current })}). Read it again and make your change on top of the current content, so the other changes are not lost.`
    } else if (
      op.tool === 'write' &&
      current !== UNHASHABLE &&
      writer &&
      writer.sessionId !== sessionId
    ) {
      reason = `${path} was written by another session, ${quoted(this.title(writer.sessionId))}, ${ago(this.now(), writer.at)}, and you have not read it. Read it before overwriting it.`
    }
    if (reason) {
      const others = writer && writer.sessionId !== sessionId
      return others
        ? `${reason} If you need to agree on who changes what, message that session with session_message (through call_tool if it is not listed).`
        : reason
    }
    pending.set(key, this.now())
    return ''
  }

  changedBy({ sessionId, viewKey, writer, current }) {
    if (writer && writer.hash === current) {
      if (writer.sessionId !== sessionId)
        return `session ${quoted(this.title(writer.sessionId))} wrote it ${ago(this.now(), writer.at)}`
      if (writer.viewKey !== viewKey) return 'another agent in this session wrote it'
    }
    return 'changed outside the file tools: a shell command, an editor, git or a formatter'
  }

  async recordResult({ sessionId, viewKey, cwd, op, failed }) {
    const file = resolveToCwd(op.path, cwd)
    const key = await fileKey(file)
    this.inflightSet(viewKey).delete(key)
    if (failed) return
    const hash = await fingerprint(file)
    const at = this.now()
    this.remember(viewKey, key, hash)
    const record = this.fileRecord(key, file)
    record.touchedBy.set(sessionId, at)
    if (op.kind !== 'write') return
    record.writer = { sessionId, viewKey, at, hash }
    const entry = this.registerSession(sessionId, cwd)
    entry.wrote.delete(key)
    entry.wrote.set(key, { file, at })
    while (entry.wrote.size > MAX_TRACKED_FILES) entry.wrote.delete(entry.wrote.keys().next().value)
    const notified = []
    for (const other of record.touchedBy.keys()) {
      if (other === sessionId) continue
      this.addNote(other, key, { from: sessionId, file, at })
      notified.push(other)
    }
    // 同一文件夹里的会话页要刷新“谁在这里改文件”。
    const neighbours = [
      ...[...this.sessions.entries()]
        .filter(([, value]) => inside(resolve(value.cwd), file))
        .map(([id]) => id),
      ...(this.sessionsNear(file) || []),
    ].filter((id) => id !== sessionId)
    this.bump([...notified, ...neighbours])
  }

  addNote(sessionId, key, note) {
    let notes = this.notes.get(sessionId)
    if (!notes) {
      notes = new Map()
      this.notes.set(sessionId, notes)
    }
    notes.delete(key)
    notes.set(key, note)
    while (notes.size > MAX_NOTE_FILES) notes.delete(notes.keys().next().value)
  }

  // 取走等着交给这个会话的提醒，拼成一段附在工具结果末尾的文字。
  takeNotes(sessionId, cwd) {
    const notes = this.notes.get(sessionId)
    if (!notes?.size) return ''
    this.notes.delete(sessionId)
    const now = this.now()
    const bySession = new Map()
    for (const note of notes.values()) {
      if (now - note.at > NOTE_TTL_MS) continue
      const list = bySession.get(note.from) || []
      list.push(note)
      bySession.set(note.from, list)
    }
    if (!bySession.size) return ''
    const lines = [...bySession.entries()].map(([from, list]) => {
      list.sort((left, right) => right.at - left.at)
      const shown = list.slice(0, NOTE_FILES_SHOWN).map((note) => displayPath(cwd, note.file))
      const more = list.length > shown.length ? ` and ${list.length - shown.length} more` : ''
      return `- Session ${quoted(this.title(from))} changed ${shown.join(', ')}${more} (${ago(now, list[0].at)}).`
    })
    return [
      '[Pisper] Another session working in the same folder changed files you have used:',
      ...lines,
      'Read them again before relying on what you saw earlier. To agree on who changes what, message that session with session_message (through call_tool if it is not listed).',
    ].join('\n')
  }

  // 最近在 cwd 里改过文件的其他会话（会话页的提示与 list_sessions 用）。
  neighbours(sessionId, cwd) {
    if (!cwd) return []
    const root = resolve(cwd)
    const now = this.now()
    const result = []
    for (const [id, entry] of this.sessions) {
      if (id === sessionId) continue
      const files = [...entry.wrote.values()].filter(
        (item) => now - item.at <= NEIGHBOUR_WINDOW_MS && inside(root, item.file),
      )
      if (!files.length) continue
      const description = this.describeSession(id)
      if (!description) continue
      files.sort((left, right) => right.at - left.at)
      result.push({
        id,
        title: description.title || '',
        running: Boolean(description.running),
        fileCount: files.length,
        files: files.slice(0, 5).map((item) => displayPath(cwd, item.file)),
        lastChangedAt: new Date(files[0].at).toISOString(),
      })
    }
    return result
      .sort((left, right) => right.lastChangedAt.localeCompare(left.lastChangedAt))
      .slice(0, 5)
  }

  recentFiles(sessionId, cwd) {
    const entry = this.sessions.get(sessionId)
    if (!entry) return []
    const now = this.now()
    return [...entry.wrote.values()]
      .filter((item) => now - item.at <= NEIGHBOUR_WINDOW_MS)
      .sort((left, right) => right.at - left.at)
      .slice(0, 5)
      .map((item) => displayPath(cwd, item.file))
  }

  // ---- 会话之间的留言 ----

  async load() {
    this.loaded ||= (async () => {
      if (!this.inboxPath) return
      const data = await readJson(this.inboxPath, null).catch(() => null)
      if (data?.version !== INBOX_VERSION || !data.inbox || typeof data.inbox !== 'object') return
      for (const [sessionId, list] of Object.entries(data.inbox)) {
        if (!Array.isArray(list)) continue
        const messages = list
          .filter(
            (item) =>
              item &&
              typeof item.id === 'string' &&
              typeof item.text === 'string' &&
              typeof item.from?.id === 'string' &&
              Number.isFinite(item.at),
          )
          .slice(-MAX_INBOX_MESSAGES)
          .map((item) => ({
            id: item.id,
            from: { id: item.from.id, title: String(item.from.title || '').slice(0, 80) },
            text: item.text.slice(0, MAX_SESSION_MESSAGE_CHARS),
            at: item.at,
          }))
        if (messages.length) this.inbox.set(sessionId, messages)
      }
    })()
    return this.loaded
  }

  save() {
    if (!this.inboxPath) return Promise.resolve()
    const snapshot = { version: INBOX_VERSION, inbox: Object.fromEntries(this.inbox) }
    const next = this.writing
      .catch(() => {})
      .then(() => writeJsonAtomic(this.inboxPath, snapshot, { mode: 0o600 }))
    this.writing = next.catch((error) => this.warn(error))
    return next
  }

  // 发话频率上限：每个会话每小时最多 SESSION_MESSAGE_LIMIT 条。
  allowSend(sessionId) {
    const now = this.now()
    const times = (this.sent.get(sessionId) || []).filter(
      (at) => now - at < SESSION_MESSAGE_WINDOW_MS,
    )
    if (times.length >= SESSION_MESSAGE_LIMIT) {
      this.sent.set(sessionId, times)
      return false
    }
    times.push(now)
    this.sent.set(sessionId, times)
    return true
  }

  newMessage({ from, text }) {
    return {
      id: randomUUID(),
      from: { id: from.id, title: String(from.title || '').slice(0, 80) },
      text: String(text || '').slice(0, MAX_SESSION_MESSAGE_CHARS),
      at: this.now(),
    }
  }

  async queueMessage(sessionId, message) {
    await this.load()
    const list = [...(this.inbox.get(sessionId) || []), message].slice(-MAX_INBOX_MESSAGES)
    this.inbox.set(sessionId, list)
    await this.save()
    this.bump([sessionId])
  }

  messages(sessionId) {
    return this.inbox.get(sessionId) || []
  }

  async takeMessages(sessionId) {
    await this.load()
    const list = this.inbox.get(sessionId) || []
    if (!list.length) return []
    this.inbox.delete(sessionId)
    await this.save()
    this.bump([sessionId])
    return list
  }

  // 会话被删除：它的留言箱与台账一起清掉。
  async forget(sessionId) {
    this.sessions.delete(sessionId)
    this.notes.delete(sessionId)
    this.views.delete(sessionId)
    this.inflight.delete(sessionId)
    for (const record of this.files.values()) record.touchedBy.delete(sessionId)
    await this.load()
    if (this.inbox.delete(sessionId)) await this.save()
  }

  // 会话页用：最近在这里改文件的会话与等着处理的留言；都没有时不带这个字段。
  projection(sessionId, cwd) {
    const neighbours = this.neighbours(sessionId, cwd)
    const messages = this.messages(sessionId).map((message) => ({
      id: message.id,
      from: message.from,
      text: message.text,
      at: new Date(message.at).toISOString(),
    }))
    return neighbours.length || messages.length ? { neighbours, messages } : null
  }
}
