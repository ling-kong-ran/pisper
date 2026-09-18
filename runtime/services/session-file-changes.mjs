// 会话文件变更服务：在没有 Git/SVN 的工作区里追踪 edit/write 工具造成的文件变动。
// 核心思路是在首次修改前把原始内容快照到数据目录，之后基于快照提供：
//   - 变更清单（新增/删除行数、待审批状态）
//   - 统一 diff 预览（复用前端 GitDiffDialog 的解析器）
//   - 一键撤销：把文件恢复到修改前内容；新建文件则删除
//   - 审批标记：用户确认过的变更不再计入待办徽标
import { createHash } from 'node:crypto'
import { mkdir, open, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises'
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

const FILE_WRITE_TOOLS = new Set(['write', 'edit'])

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
    this.wrapped = new WeakSet()
    // 每个会话的条目索引按 sessionId 缓存，写操作串行化后落盘。
    this.entries = new Map()
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
    return list
  }

  save(sessionId, list) {
    this.entries.set(sessionId, list)
    const previous = this.writing.get(sessionId) || Promise.resolve()
    const next = previous
      .catch(() => {})
      .then(() => writeJsonAtomic(this.indexPath(sessionId), { entries: list }))
      .catch((error) => this.warn(error))
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
        scored.push({ name: dir.name, mtime: (await stat(join(this.root, dir.name))).mtimeMs })
      } catch {
        // 目录可能刚好被清理，跳过即可。
      }
    }
    scored.sort((a, b) => b.mtime - a.mtime)
    for (const stale of scored.slice(MAX_SNAPSHOT_SESSIONS)) {
      await rm(join(this.root, stale.name), { recursive: true, force: true }).catch(() => {})
    }
  }

  // 在 Agent 会话上安装 beforeToolCall 钩子：对被允许的 write/edit 调用，
  // 在执行前完成原始内容快照，执行成功后登记变更条目。
  install(session, { sessionId, cwd }) {
    if (!session?.agent || !cwd || !sessionId || this.installed.has(session)) return
    this.installed.add(session)
    const previous = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      // 先保留既有审批/文件范围检查；被拒绝的调用不会建立快照。
      const decision = await previous?.(context, signal)
      if (decision?.block || signal?.aborted) return decision
      const name = context.toolCall.name
      if (!writeOperation(name, context.args)) return decision
      const tool = context.context?.tools?.find((item) => item.name === name)
      if (tool && !this.wrapped.has(tool)) {
        this.wrapped.add(tool)
        const execute = tool.execute
        tool.execute = (...args) =>
          this.run({ sessionId, cwd, name, args: args[1] }, () => execute.apply(tool, args))
      }
      return decision
    }
  }

  async run({ sessionId, cwd, name, args }, execute) {
    const op = writeOperation(name, args)
    if (!op) return execute()
    const absolutePath = resolveToCwd(op.path, cwd)
    if (!nested(cwd, absolutePath)) return execute()
    return this.serialize(sessionId, async () => {
      try {
        // 快照必须在写入前完成；同一文件多次编辑只保留最初版本，撤销才能回到起点。
        await this.captureBefore(sessionId, cwd, absolutePath)
      } catch (error) {
        // 快照失败不阻断工具执行，只是该文件失去 diff/撤销能力。
        this.warn(error)
      }
      const result = await execute()
      try {
        await this.recordChange(sessionId, cwd, absolutePath)
      } catch (error) {
        this.warn(error)
      }
      return result
    })
  }

  async captureBefore(sessionId, cwd, absolutePath) {
    const list = await this.load(sessionId)
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
