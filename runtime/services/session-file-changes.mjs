// 会话文件变更服务：在没有 Git/SVN 的工作区里追踪 edit/write 工具造成的文件变动。
// 核心思路是在首次修改前把原始内容快照到数据目录，之后基于快照提供：
//   - 变更清单（新增/删除行数、待审批状态）
//   - 统一 diff 预览（复用前端 GitDiffDialog 的解析器）
//   - 一键撤销：把文件恢复到修改前内容；新建文件则删除
//   - 审批标记：用户确认过的变更不再计入待办徽标
import { createHash } from 'node:crypto'
import {
  lstat,
  mkdir,
  open,
  readdir,
  readFile,
  realpath,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import {
  generateUnifiedPatch,
  normalizeToLF,
  resolveToCwd,
  stripBom,
} from '../runtime/pi-coding-agent.mjs'

// 快照大小上限：超过后只记录变动事实，不提供 diff/撤销。
export const MAX_SNAPSHOT_BYTES = 4 * 1024 * 1024
// 单次 diff 输出上限，避免超大文件撑爆前端渲染。
export const MAX_DIFF_CHARS = 200_000
// 单个会话最多记录的变更文件数，防止失控写入撑爆存储。
const MAX_ENTRIES_PER_SESSION = 200
// 最多保留多少个会话的快照目录，超出按修改时间淘汰最旧。
const MAX_SNAPSHOT_SESSIONS = 50
// 空索引仅证明新会话尚无写入，不应挤占可撤销的真实快照名额。
const MAX_EMPTY_SESSION_MARKERS = 500
// 会话摘要只用于导航徽标，限制一次读取与差异计算，避免大文件拖慢会话页。
const MAX_SUMMARY_FILE_BYTES = 512 * 1024
const MAX_SUMMARY_TOTAL_BYTES = 2 * 1024 * 1024
const MAX_SUMMARY_INDEX_BYTES = 512 * 1024
const MAX_SUMMARY_DIFF_LINES = 2_000
const MAX_EMPTY_MARKER_INDEX_BYTES = 8 * 1024

const FILE_WRITE_TOOLS = new Set(['write', 'edit'])
// 仅对已知绝不写入工作区的 Pi 内置工具保留完整覆盖；扩展/MCP/命令工具默认未知。
const READ_ONLY_TOOLS = new Set(['read', 'grep', 'find', 'ls'])
const INDEX_VERSION = 2

function hashKey(value) {
  return createHash('sha256').update(value).digest('hex')
}

// 与网关的名称归一化保持一致，兼容 call_tool 透传调用。
function writeOperation(name, args) {
  if (name === 'call_tool') {
    name = String(args?.name || '').trim()
    args = args?.arguments
  }
  if (!FILE_WRITE_TOOLS.has(name)) return null
  const path = typeof args?.path === 'string' ? args.path.trim() : ''
  return path ? { path } : null
}

function effectiveToolName(name, args) {
  return name === 'call_tool' ? String(args?.name || '').trim() : name
}

function nested(root, path) {
  const child = relative(root, path)
  return !isAbsolute(child) && child !== '..' && !child.startsWith(`..${sep}`)
}

// 探测文件是否适合文本快照：只看前 8KB 是否含 NUL，且总大小不超上限。
async function snapshotContent(absolutePath) {
  const fileStat = await stat(absolutePath)
  if (!fileStat.isFile()) return { ok: false, reason: 'not-file' }
  if (fileStat.size > MAX_SNAPSHOT_BYTES) return { ok: false, reason: 'too-large' }
  const handle = await open(absolutePath, 'r')
  try {
    const probe = Buffer.alloc(Math.min(8192, fileStat.size))
    await handle.read(probe, 0, probe.length, 0)
    if (probe.subarray(0, probe.length).includes(0)) return { ok: false, reason: 'binary' }
  } finally {
    await handle.close()
  }
  const content = await readFile(absolutePath, 'utf8')
  return { ok: true, content }
}

function diffStats(patch) {
  let added = 0
  let removed = 0
  for (const line of patch.split('\n')) {
    if (line.startsWith('+') && !line.startsWith('+++')) added += 1
    else if (line.startsWith('-') && !line.startsWith('---')) removed += 1
  }
  return { added, removed }
}

function truncateDiff(diff) {
  if (diff.length <= MAX_DIFF_CHARS) return { diff, truncated: false }
  return { diff: diff.slice(0, MAX_DIFF_CHARS), truncated: true }
}

function unavailableSummary() {
  return {
    status: 'unavailable',
    changedFiles: null,
    pendingFiles: null,
    added: null,
    removed: null,
    unknownFiles: 0,
    capped: false,
  }
}

function partialSummary(unknownFiles, capped) {
  return {
    status: 'partial',
    changedFiles: null,
    pendingFiles: null,
    added: null,
    removed: null,
    unknownFiles,
    capped,
  }
}

function hasTooManyLines(text) {
  let lines = 1
  for (const char of text) {
    if (char === '\n' && ++lines > MAX_SUMMARY_DIFF_LINES) return true
  }
  return false
}

// 摘要读取只接受工作区/快照目录内的普通文本文件；无法确认时交给 partial，
// 不把超限、二进制或越界符号链接误算成零改动。
async function readSummaryText(root, target, budget) {
  let actual
  try {
    actual = await realpath(target)
  } catch (error) {
    return error?.code === 'ENOENT' ? { kind: 'missing' } : { kind: 'unknown' }
  }
  if (!nested(root, actual)) return { kind: 'unknown' }
  try {
    const info = await stat(actual)
    if (!info.isFile() || info.size > MAX_SUMMARY_FILE_BYTES || info.size > budget.remaining)
      return { kind: 'unknown' }
    // stat 后文件仍可能增长；固定缓冲区最多多读一字节，避免竞态下越过预算。
    const handle = await open(actual, 'r')
    let content
    try {
      const buffer = Buffer.alloc(info.size + 1)
      let read = 0
      while (read < buffer.length) {
        const result = await handle.read(buffer, read, buffer.length - read, read)
        if (!result.bytesRead) break
        read += result.bytesRead
      }
      if (read !== info.size) return { kind: 'unknown' }
      content = buffer.subarray(0, read)
    } finally {
      await handle.close()
    }
    budget.remaining -= content.length
    if (content.includes(0)) return { kind: 'unknown' }
    return { kind: 'file', text: normalizeToLF(stripBom(content.toString('utf8')).text) }
  } catch (error) {
    return error?.code === 'ENOENT' ? { kind: 'missing' } : { kind: 'unknown' }
  }
}

// Pi 已生成文件头，只替换路径；相同内容不能用仅含文件头的补丁冒充改动。
function snapshotFileDiff(path, before, after, isNew) {
  if (before === after) return ''
  const quote = (value) => (/\s|"|\\/.test(value) ? JSON.stringify(value) : value)
  const oldPath = quote(`a/${path}`)
  const newPath = quote(`b/${path}`)
  const patch = generateUnifiedPatch(path, before, after)
    .replace(/^--- .*$/m, () => `--- ${isNew ? '/dev/null' : oldPath}`)
    .replace(/^\+\+\+ .*$/m, () => `+++ ${newPath}`)
  return `diff --git ${oldPath} ${newPath}\n${isNew ? 'new file mode 100644\n' : ''}${patch}`
}

export class SessionFileChangesService {
  constructor({ dataDir, warn = (error) => console.warn('文件变更快照待重试：', error.message) }) {
    this.root = resolve(dataDir, 'file-change-snapshots')
    this.warn = warn
    this.installed = new WeakSet()
    this.wrapped = new WeakMap()
    // 每个会话的条目索引按 sessionId 缓存，写操作串行化后落盘。
    this.entries = new Map()
    this.indexMeta = new Map()
    this.writing = new Map()
    // Pi 可并行执行同一会话的工具调用；快照与落盘必须串行，才能始终保留首次修改前的版本。
    this.running = new Map()
    this.pruned = this.pruneOldSessions().catch(() => {})
  }

  sessionDir(sessionId) {
    return join(this.root, hashKey(String(sessionId)).slice(0, 32))
  }

  indexPath(sessionId) {
    return join(this.sessionDir(sessionId), 'index.json')
  }

  snapshotPath(sessionId, key) {
    return join(this.sessionDir(sessionId), `${key}.before`)
  }

  async load(sessionId) {
    if (this.entries.has(sessionId)) return this.entries.get(sessionId)
    const data = await readJson(this.indexPath(sessionId), { entries: [] })
    const list = Array.isArray(data.entries) ? data.entries : []
    this.entries.set(sessionId, list)
    if (
      data.version === INDEX_VERSION &&
      typeof data.cwd === 'string' &&
      (data.coverage === 'complete' || data.coverage === 'partial')
    )
      this.indexMeta.set(sessionId, {
        version: INDEX_VERSION,
        cwd: data.cwd,
        coverage: data.coverage,
      })
    return list
  }

  // 新会话创建时由调用方显式建立空索引：有索引的零写入才可证明为零改动。
  // 历史会话缺少索引可能是 50 会话快照淘汰，不能补建或推断为零。
  async markSessionTracked(sessionId, cwd) {
    if (typeof cwd !== 'string' || !cwd) throw new Error('会话工作区路径不能为空。')
    await this.pruned
    return this.serialize(sessionId, async () => {
      try {
        await stat(this.indexPath(sessionId))
        return
      } catch (error) {
        if (error?.code !== 'ENOENT') throw error
      }
      const meta = {
        version: INDEX_VERSION,
        cwd: await realpath(cwd),
        coverage: 'complete',
      }
      await writeJsonAtomic(this.indexPath(sessionId), { ...meta, entries: [] })
      this.entries.set(sessionId, [])
      this.indexMeta.set(sessionId, meta)
    })
  }

  // 未快照覆盖的工具可能写任意文件；执行前持久降级，重启后也不能误报精确零。
  async markCoveragePartial(sessionId, cwd) {
    await this.pruned
    return this.serialize(sessionId, () => this.persistPartialCoverage(sessionId, cwd))
  }

  async persistPartialCoverage(sessionId, cwd) {
    const list = await this.load(sessionId)
    const prior = this.indexMeta.get(sessionId)
    if (prior?.coverage === 'partial') return
    await this.writing.get(sessionId)?.catch(() => {})
    const meta = {
      version: INDEX_VERSION,
      cwd: prior?.cwd || (await realpath(cwd)),
      coverage: 'partial',
    }
    await writeJsonAtomic(this.indexPath(sessionId), { ...meta, entries: list })
    this.indexMeta.set(sessionId, meta)
  }

  save(sessionId, list) {
    this.entries.set(sessionId, list)
    const previous = this.writing.get(sessionId) || Promise.resolve()
    const next = previous
      .catch(() => {})
      .then(() =>
        writeJsonAtomic(this.indexPath(sessionId), {
          ...this.indexMeta.get(sessionId),
          entries: list,
        }),
      )
      .catch((error) => {
        this.warn(error)
        throw error
      })
    this.writing.set(sessionId, next)
    return next
  }

  // 同一会话内写工具串行，防止并发调用互相覆盖首次快照或索引。
  async serialize(sessionId, task) {
    const previous = this.running.get(sessionId) || Promise.resolve()
    let release
    const pending = new Promise((resolve) => {
      release = resolve
    })
    const current = previous.catch(() => {}).then(() => pending)
    this.running.set(sessionId, current)
    await previous.catch(() => {})
    try {
      return await task()
    } finally {
      release()
      if (this.running.get(sessionId) === current) this.running.delete(sessionId)
    }
  }

  // 淘汰最旧的快照会话，避免数据目录无限增长。
  async pruneOldSessions() {
    let dirs
    try {
      dirs = await readdir(this.root, { withFileTypes: true })
    } catch (error) {
      if (error?.code === 'ENOENT') return
      throw error
    }
    const sessionDirs = dirs.filter((dir) => dir.isDirectory())
    if (sessionDirs.length <= MAX_SNAPSHOT_SESSIONS) return
    const scored = []
    for (const dir of sessionDirs) {
      try {
        const path = join(this.root, dir.name)
        const mtime = (await stat(path)).mtimeMs
        let emptyMarker = false
        const indexPath = join(path, 'index.json')
        const indexStat = await lstat(indexPath).catch(() => null)
        if (indexStat?.isFile() && indexStat.size <= MAX_EMPTY_MARKER_INDEX_BYTES) {
          const index = await readJson(indexPath, null).catch(() => null)
          emptyMarker = Array.isArray(index?.entries) && index.entries.length === 0
        }
        scored.push({ name: dir.name, mtime, emptyMarker })
      } catch {
        // 目录可能刚好被清理，跳过即可。
      }
    }
    const newestFirst = (a, b) => b.mtime - a.mtime
    const snapshots = scored.filter((dir) => !dir.emptyMarker).sort(newestFirst)
    const markers = scored.filter((dir) => dir.emptyMarker).sort(newestFirst)
    for (const stale of [
      ...snapshots.slice(MAX_SNAPSHOT_SESSIONS),
      ...markers.slice(MAX_EMPTY_SESSION_MARKERS),
    ]) {
      await rm(join(this.root, stale.name), { recursive: true, force: true }).catch(() => {})
    }
  }

  // 在 Agent 会话上安装 beforeToolCall 钩子：write/edit 执行前保存快照，
  // 其他可能写入的工具执行前持久标记摘要覆盖不完整。
  install(session, { sessionId, cwd }) {
    if (!session?.agent || !cwd || !sessionId || this.installed.has(session)) return
    this.installed.add(session)
    const previous = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      // 先保留既有审批/文件范围检查；被拒绝的调用不会建立快照。
      const decision = await previous?.(context, signal)
      if (decision?.block || signal?.aborted) return decision
      const name = context.toolCall.name
      if (!writeOperation(name, context.args)) {
        if (name === 'call_tool' || !READ_ONLY_TOOLS.has(effectiveToolName(name, context.args)))
          await this.markCoveragePartial(sessionId, cwd)
        return decision
      }
      const tool = context.context?.tools?.find((item) => item.name === name)
      if (!tool) {
        await this.markCoveragePartial(sessionId, cwd)
        return decision
      }
      const wrappedFor = this.wrapped.get(tool)
      if (wrappedFor && (wrappedFor.sessionId !== sessionId || wrappedFor.cwd !== cwd)) {
        // 共享工具实例已绑定其他会话，不能把它的写入归到当前会话的完整快照。
        await this.markCoveragePartial(sessionId, cwd)
        await this.markCoveragePartial(wrappedFor.sessionId, wrappedFor.cwd)
        return decision
      }
      if (!wrappedFor) {
        this.wrapped.set(tool, { sessionId, cwd })
        const execute = tool.execute
        tool.execute = (...args) =>
          this.run({ sessionId, cwd, name, args: args[1] }, () => execute.apply(tool, args))
      }
      return decision
    }
  }

  async run({ sessionId, cwd, name, args }, execute) {
    const op = writeOperation(name, args)
    if (!op) {
      await this.markCoveragePartial(sessionId, cwd)
      return execute()
    }
    const absolutePath = resolveToCwd(op.path, cwd)
    if (!nested(cwd, absolutePath)) {
      await this.markCoveragePartial(sessionId, cwd)
      return execute()
    }
    return this.serialize(sessionId, async () => {
      try {
        // 快照必须在写入前完成；同一文件多次编辑只保留最初版本，撤销才能回到起点。
        await this.captureBefore(sessionId, cwd, absolutePath)
      } catch (error) {
        // 快照失败时先持久降级；若连降级标记也无法写入，则不能继续执行并误报零改动。
        this.warn(error)
        await this.persistPartialCoverage(sessionId, cwd)
      }
      const result = await execute()
      try {
        await this.recordChange(sessionId, cwd, absolutePath)
      } catch (error) {
        this.warn(error)
        await this.persistPartialCoverage(sessionId, cwd)
      }
      return result
    })
  }

  async captureBefore(sessionId, cwd, absolutePath) {
    const list = await this.load(sessionId)
    const meta = this.indexMeta.get(sessionId)
    if (!meta || meta.cwd !== (await realpath(cwd)))
      await this.persistPartialCoverage(sessionId, cwd)
    const relPath = relative(cwd, absolutePath).replace(/\\/g, '/')
    if (list.some((entry) => entry.path === relPath)) return
    const key = hashKey(relPath).slice(0, 24)
    let beforeExists = false
    let snapshot = false
    try {
      const source = await snapshotContent(absolutePath)
      beforeExists = true
      if (source.ok) {
        await mkdir(this.sessionDir(sessionId), { recursive: true })
        await writeFile(this.snapshotPath(sessionId, key), source.content, 'utf8')
        snapshot = true
      }
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    if (list.length >= MAX_ENTRIES_PER_SESSION) return
    list.push({
      path: relPath,
      key,
      beforeExists,
      snapshot,
      changeCount: 0,
      approved: false,
      reverted: false,
      changedAt: '',
    })
    await this.save(sessionId, list)
  }

  async recordChange(sessionId, cwd, absolutePath) {
    const list = await this.load(sessionId)
    const relPath = relative(cwd, absolutePath).replace(/\\/g, '/')
    const entry = list.find((item) => item.path === relPath)
    if (!entry) return
    entry.changeCount += 1
    entry.approved = false
    entry.reverted = false
    entry.changedAt = new Date().toISOString()
    await this.save(sessionId, list)
  }

  // 读取快照原始内容；文件新建（beforeExists=false）返回空串语义由调用方处理。
  async readSnapshot(sessionId, entry) {
    if (!entry.snapshot) return null
    try {
      return await readFile(this.snapshotPath(sessionId, entry.key), 'utf8')
    } catch (error) {
      if (error?.code === 'ENOENT') return null
      throw error
    }
  }

  async readCurrent(absolutePath) {
    try {
      const content = await readFile(absolutePath, 'utf8')
      const { text } = stripBom(content)
      return { exists: true, content: normalizeToLF(text) }
    } catch (error) {
      if (error?.code === 'ENOENT' || error?.code === 'EISDIR')
        return { exists: false, content: '' }
      throw error
    }
  }

  // 导航只展示“当前仍与首次修改前不同”的会话内文件。pendingFiles 是其中尚未
  // 审批的数量；added/removed 是与首次修改前比较的行数。unknownFiles 只数索引
  // 中无法核对的条目，capped 表示 200 条上限还可能漏记更多文件。没有索引可能是
  // 旧快照已淘汰，不能当作零；partial/unavailable 时数值一律为 null。
  async summary(sessionId, cwd) {
    await this.pruned
    await this.writing.get(sessionId)?.catch(() => {})
    const indexPath = this.indexPath(sessionId)
    let data
    try {
      const info = await stat(indexPath)
      if (!info.isFile() || info.size > MAX_SUMMARY_INDEX_BYTES) return unavailableSummary()
      data = JSON.parse(await readFile(indexPath, 'utf8'))
    } catch {
      return unavailableSummary()
    }
    if (!Array.isArray(data?.entries)) return unavailableSummary()

    const entries = data.entries
    const capped = entries.length >= MAX_ENTRIES_PER_SESSION
    const cached = this.entries.get(sessionId)
    if (cached && JSON.stringify(cached) !== JSON.stringify(entries))
      return partialSummary(Math.max(cached.length, entries.length), capped)
    const cachedMeta = this.indexMeta.get(sessionId)
    if (
      cachedMeta &&
      (cachedMeta.version !== data.version ||
        cachedMeta.cwd !== data.cwd ||
        cachedMeta.coverage !== data.coverage)
    )
      return partialSummary(entries.length, capped)
    // 旧索引缺少起始工作区和覆盖状态，无法证明空目录或相对路径的含义。
    if (
      data.version !== INDEX_VERSION ||
      typeof data.cwd !== 'string' ||
      !data.cwd ||
      (data.coverage !== 'complete' && data.coverage !== 'partial')
    )
      return partialSummary(entries.length, capped)
    let cwdRoot
    try {
      cwdRoot = await realpath(cwd)
    } catch {
      return partialSummary(entries.length, capped)
    }
    if (cwdRoot !== data.cwd || data.coverage === 'partial')
      return partialSummary(entries.length, capped)
    if (!entries.length)
      return {
        status: 'known',
        changedFiles: 0,
        pendingFiles: 0,
        added: 0,
        removed: 0,
        unknownFiles: 0,
        capped: false,
      }
    const budget = { remaining: MAX_SUMMARY_TOTAL_BYTES }
    let snapshotRoot
    try {
      snapshotRoot = await realpath(this.sessionDir(sessionId))
    } catch {
      return partialSummary(entries.length, capped)
    }

    let changedFiles = 0
    let pendingFiles = 0
    let added = 0
    let removed = 0
    let unknownFiles = Math.max(0, entries.length - MAX_ENTRIES_PER_SESSION)
    const seen = new Set()
    for (const entry of entries.slice(0, MAX_ENTRIES_PER_SESSION)) {
      if (
        !entry ||
        typeof entry.path !== 'string' ||
        !entry.path ||
        typeof entry.key !== 'string' ||
        !/^[a-f0-9]{24}$/.test(entry.key) ||
        typeof entry.beforeExists !== 'boolean' ||
        seen.has(entry.path)
      ) {
        unknownFiles += 1
        continue
      }
      seen.add(entry.path)
      const absolutePath = resolve(cwd, entry.path)
      if (!nested(resolve(cwd), absolutePath)) {
        unknownFiles += 1
        continue
      }
      const current = await readSummaryText(cwdRoot, absolutePath, budget)
      const before = entry.beforeExists
        ? entry.snapshot
          ? await readSummaryText(snapshotRoot, this.snapshotPath(sessionId, entry.key), budget)
          : { kind: 'unknown' }
        : { kind: 'missing' }
      if (current.kind === 'unknown' || (entry.beforeExists && before.kind !== 'file')) {
        unknownFiles += 1
        continue
      }
      if (!entry.beforeExists && current.kind === 'missing') continue
      const previousText = before.kind === 'file' ? before.text : ''
      const currentText = current.kind === 'file' ? current.text : ''
      if (entry.beforeExists && current.kind === 'file' && currentText === previousText) continue
      if (hasTooManyLines(previousText) || hasTooManyLines(currentText)) {
        unknownFiles += 1
        continue
      }
      let stats
      try {
        stats = diffStats(generateUnifiedPatch(entry.path, previousText, currentText))
      } catch {
        unknownFiles += 1
        continue
      }
      changedFiles += 1
      if (!entry.approved) pendingFiles += 1
      added += stats.added
      removed += stats.removed
    }

    if (unknownFiles || capped) return partialSummary(unknownFiles, capped)
    return { status: 'known', changedFiles, pendingFiles, added, removed, unknownFiles: 0, capped }
  }

  // 变更清单：实时对比快照与磁盘现状，计算行数统计与待审批数量。
  async list(sessionId, cwd) {
    await this.pruned
    const list = await this.load(sessionId)
    const files = []
    let added = 0
    let removed = 0
    for (const entry of list) {
      const absolutePath = resolve(cwd, entry.path)
      const current = await this.readCurrent(absolutePath)
      const before = await this.readSnapshot(sessionId, entry)
      const snapshotBeforeExists = entry.beforeExists && before !== null
      let entryAdded = 0
      let entryRemoved = 0
      let currentEqualsBefore = false
      if (before !== null) {
        const stats = diffStats(generateUnifiedPatch(entry.path, before, current.content))
        entryAdded = stats.added
        entryRemoved = stats.removed
        currentEqualsBefore = current.exists === snapshotBeforeExists && current.content === before
      } else if (!entry.beforeExists) {
        // 无快照的新建文件：按当前行数统计新增。
        entryAdded = current.exists ? current.content.split('\n').length : 0
        currentEqualsBefore = !current.exists
      }
      // 文件状态取决于写入前是否存在，不能因二进制/过大而缺失快照就误报为新建。
      const status =
        !entry.beforeExists && current.exists
          ? 'created'
          : entry.beforeExists && !current.exists
            ? 'deleted'
            : 'modified'
      const reverted = Boolean(entry.reverted) || currentEqualsBefore
      const pending =
        !reverted && !entry.approved && (entryAdded > 0 || entryRemoved > 0 || before === null)
      added += entryAdded
      removed += entryRemoved
      files.push({
        path: entry.path,
        status,
        added: entryAdded,
        removed: entryRemoved,
        changeCount: entry.changeCount,
        snapshot: before !== null,
        canRevert: before !== null || !entry.beforeExists,
        approved: Boolean(entry.approved),
        reverted,
        pending,
        changedAt: entry.changedAt,
        currentEqualsBefore,
      })
    }
    files.sort((a, b) => b.changedAt.localeCompare(a.changedAt))
    return {
      files,
      summary: {
        files: files.length,
        pending: files.filter((file) => file.pending).length,
        added,
        removed,
      },
    }
  }

  // 单文件 diff：快照 → 当前内容。新建文件以空内容为基线；
  // 既有文件无快照（超大/二进制）时返回空 diff。
  async diff(sessionId, cwd, relPath) {
    const list = await this.load(sessionId)
    const entry = list.find((item) => item.path === relPath)
    if (!entry) return { diff: '', diffTruncated: false, source: 'snapshot', found: false }
    const current = await this.readCurrent(resolve(cwd, entry.path))
    let before = await this.readSnapshot(sessionId, entry)
    if (before === null && !entry.beforeExists) before = ''
    if (before === null) return { diff: '', diffTruncated: false, source: 'snapshot', found: true }
    const text = snapshotFileDiff(entry.path, before, current.content, !entry.beforeExists)
    const result = truncateDiff(text)
    return { diff: result.diff, diffTruncated: result.truncated, source: 'snapshot', found: true }
  }

  // 撤销：把目标文件恢复到修改前（有快照写回内容；新建文件删除）。
  // path 缺省时撤销该会话全部可撤销的变更。返回是否确实有变更被恢复。
  async revert(sessionId, cwd, relPath) {
    const list = await this.load(sessionId)
    const targets = list.filter((entry) => (relPath ? entry.path === relPath : true))
    let reverted = 0
    for (const entry of targets) {
      const before = await this.readSnapshot(sessionId, entry)
      const absolutePath = resolve(cwd, entry.path)
      if (before !== null) {
        await mkdir(dirname(absolutePath), { recursive: true })
        await writeFile(absolutePath, before, 'utf8')
        reverted += 1
        entry.reverted = true
        entry.approved = true
      } else if (!entry.beforeExists) {
        await rm(absolutePath, { force: true })
        reverted += 1
        entry.reverted = true
        entry.approved = true
      }
      // 二进制/超大等无快照的既有文件无法自动恢复，保持未撤销状态。
    }
    if (reverted > 0 || targets.length > 0) await this.save(sessionId, list)
    return { reverted, files: await this.list(sessionId, cwd) }
  }

  // 审批：标记用户已确认（可选单个文件），待办徽标随之清零。
  async approve(sessionId, cwd, relPath) {
    const list = await this.load(sessionId)
    const targets = list.filter((entry) => (relPath ? entry.path === relPath : true))
    for (const entry of targets) entry.approved = true
    if (targets.length) await this.save(sessionId, list)
    return { files: await this.list(sessionId, cwd) }
  }

  async clear(sessionId) {
    // 删除会话前先等待已开始的快照与索引写入，避免延迟写重新创建已清理目录。
    await this.running.get(sessionId)?.catch(() => {})
    await this.writing.get(sessionId)?.catch(() => {})
    this.entries.delete(sessionId)
    this.writing.delete(sessionId)
    await rm(this.sessionDir(sessionId), { recursive: true, force: true })
  }
}
