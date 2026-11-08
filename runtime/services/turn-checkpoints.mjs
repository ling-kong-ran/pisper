// 按轮回退文件：write/edit 工具执行前，把目标文件当时的内容存一份；回退到某个时间点时，
// 每个文件取“该时间点之后第一次被改之前”的内容写回，当时还不存在的文件删除。
// 只覆盖 Pisper 文件工具（写入/编辑，含子 Agent，归属父会话）；bash、MCP 等其它途径的改动不在其中，
// 预览和确认里必须如实说明。快照放在数据目录，按内容哈希去重，不碰工作区和用户的 Git 仓库。
//
// 回退不盖掉别人的修改：每次写入后记下文件留下的内容（written）；回退时磁盘上的内容已经不是
// 本会话最后写下的（隔壁会话、命令行或编辑器之后又改过），这个文件就不回退，在预览里列出来。
//
// 去重规则：同一文件在每条用户消息之后只记第一次修改前的内容（markTurn 记下最近一条用户消息的时间）。
// 回退目标总是某条消息的时间，而它之后对每个文件的第一次修改一定落在某条用户消息之后的“第一次”里，
// 所以取目标时间之后最早的那份快照就是该文件在目标时间的内容。
import { createHash } from 'node:crypto'
import {
  lstat,
  mkdir,
  readdir,
  readFile,
  realpath,
  rm,
  stat,
  unlink,
  writeFile,
} from 'node:fs/promises'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { writeOperation } from './session-file-changes.mjs'
import { ABSENT, UNHASHABLE, fingerprint } from './workspace-coordination.mjs'
import { resolveToCwd } from '../runtime/pi-coding-agent.mjs'

export const MAX_CHECKPOINT_FILE_BYTES = 4 * 1024 * 1024
// 单个会话最多保留的快照条数；更早的被淘汰后，回退到那之前时如实提示不完整。
const MAX_SNAPSHOTS_PER_SESSION = 2000
const MAX_CHECKPOINT_SESSIONS = 50
const MAX_WRITTEN_FILES = 5000
const INDEX_VERSION = 1

function sha256(value) {
  return createHash('sha256').update(value).digest('hex')
}

function nested(root, path) {
  const child = relative(root, path)
  return Boolean(child) && !isAbsolute(child) && child !== '..' && !child.startsWith(`..${sep}`)
}

// 写回前确认真实路径仍在工作区内：父目录被换成指向外部的符号链接时不写。
async function realInside(root, file) {
  const realRoot = await realpath(root).catch(() => null)
  if (!realRoot) return false
  let ancestor = dirname(file)
  for (;;) {
    const real = await realpath(ancestor).catch(() => null)
    if (real) return real === realRoot || nested(realRoot, real)
    const parent = dirname(ancestor)
    if (parent === ancestor) return false
    ancestor = parent
  }
}

function emptyIndex() {
  return { version: INDEX_VERSION, mark: 0, truncatedBefore: 0, snapshots: [], written: {} }
}

// 旧索引没有 written：那些文件照旧回退（不知道最后写下的是什么）。
function writtenFiles(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
  return Object.fromEntries(
    Object.entries(value).filter(
      ([file, hash]) => isAbsolute(file) && (hash === null || typeof hash === 'string'),
    ),
  )
}

export class TurnCheckpointService {
  constructor({ dataDir, now = () => Date.now(), warn = () => {} }) {
    this.root = join(dataDir, 'turn-checkpoints')
    this.now = now
    this.warn = warn
    this.indexes = new Map()
    this.queues = new Map()
    this.installed = new WeakSet()
  }

  sessionDir(sessionId) {
    return join(this.root, sha256(String(sessionId)).slice(0, 32))
  }

  blobPath(sessionId, hash) {
    return join(this.sessionDir(sessionId), 'blobs', hash)
  }

  async load(sessionId) {
    if (this.indexes.has(sessionId)) return this.indexes.get(sessionId)
    const data = await readJson(join(this.sessionDir(sessionId), 'index.json'), null).catch(
      () => null,
    )
    const index =
      data?.version === INDEX_VERSION && Array.isArray(data.snapshots)
        ? {
            ...emptyIndex(),
            mark: Number(data.mark) || 0,
            truncatedBefore: Number(data.truncatedBefore) || 0,
            snapshots: data.snapshots.filter(
              (item) =>
                item &&
                typeof item.file === 'string' &&
                typeof item.cwd === 'string' &&
                Number.isFinite(item.at),
            ),
            written: writtenFiles(data.written),
          }
        : emptyIndex()
    this.indexes.set(sessionId, index)
    return index
  }

  async save(sessionId, index) {
    await mkdir(this.sessionDir(sessionId), { recursive: true, mode: 0o700 })
    await writeJsonAtomic(join(this.sessionDir(sessionId), 'index.json'), index)
  }

  // 同一会话的快照与回退串行执行，避免并行工具调用读写同一份索引。
  serialize(sessionId, task) {
    const previous = this.queues.get(sessionId) || Promise.resolve()
    const current = previous.catch(() => {}).then(task)
    const settled = current.catch(() => {})
    this.queues.set(sessionId, settled)
    void settled.then(() => {
      if (this.queues.get(sessionId) === settled) this.queues.delete(sessionId)
    })
    return current
  }

  // 一条新的用户消息开始：之后的修改重新记“第一次修改前”的内容。
  markTurn(sessionId, at) {
    const time = Number(at) || this.now()
    void this.serialize(sessionId, async () => {
      const index = await this.load(sessionId)
      if (time > index.mark) index.mark = time
    })
  }

  // 在 Agent 会话上挂 beforeToolCall：审批通过后、write/edit 真正执行前记快照。
  install(session, { sessionId, cwd }) {
    if (!session?.agent || !cwd || !sessionId || this.installed.has(session)) return
    this.installed.add(session)
    const previous = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      const decision = await previous?.(context, signal)
      if (decision?.block || signal?.aborted) return decision
      const op = writeOperation(context.toolCall?.name, context.args)
      if (!op) return decision
      try {
        await this.capture(sessionId, cwd, resolveToCwd(op.path, cwd))
      } catch (error) {
        // 快照失败不拦截写入，只是这个文件之后不能回退；预览会把它列为无法回退。
        this.warn(error)
      }
      return decision
    }
    const after = session.agent.afterToolCall
    session.agent.afterToolCall = async (context, signal) => {
      const override = await after?.(context, signal)
      const op = writeOperation(context.toolCall?.name, context.args)
      if (op && !(override?.isError ?? context.isError))
        await this.recordWritten(sessionId, cwd, resolveToCwd(op.path, cwd)).catch((error) =>
          this.warn(error),
        )
      return override
    }
  }

  // 写入之后文件留下的内容：回退时据此判断之后有没有被别人改过。
  recordWritten(sessionId, cwd, file) {
    const root = resolve(cwd)
    const target = resolve(file)
    if (!nested(root, target)) return Promise.resolve()
    return this.serialize(sessionId, async () => {
      const index = await this.load(sessionId)
      const hash = await fingerprint(target)
      delete index.written[target]
      index.written[target] = hash === ABSENT ? null : hash
      const files = Object.keys(index.written)
      for (const stale of files.slice(0, Math.max(0, files.length - MAX_WRITTEN_FILES)))
        delete index.written[stale]
      await this.save(sessionId, index)
    })
  }

  capture(sessionId, cwd, file) {
    const root = resolve(cwd)
    const target = resolve(file)
    if (!nested(root, target)) return Promise.resolve()
    return this.serialize(sessionId, async () => {
      const index = await this.load(sessionId)
      if (index.snapshots.some((item) => item.file === target && item.at >= index.mark)) return
      const snapshot = { at: this.now(), cwd: root, file: target, existed: false, blob: null }
      const info = await lstat(target).catch((error) => {
        if (error?.code === 'ENOENT') return null
        throw error
      })
      if (info) {
        snapshot.existed = true
        if (!info.isFile()) snapshot.reason = 'not-file'
        else if (info.size > MAX_CHECKPOINT_FILE_BYTES) snapshot.reason = 'too-large'
        else {
          const content = await readFile(target)
          snapshot.blob = sha256(content)
          const path = this.blobPath(sessionId, snapshot.blob)
          if (!(await stat(path).catch(() => null))) {
            await mkdir(dirname(path), { recursive: true, mode: 0o700 })
            await writeFile(path, content, { mode: 0o600 })
          }
        }
      }
      index.snapshots.push(snapshot)
      if (index.snapshots.length > MAX_SNAPSHOTS_PER_SESSION) {
        const dropped = index.snapshots.splice(
          0,
          index.snapshots.length - MAX_SNAPSHOTS_PER_SESSION,
        )
        index.truncatedBefore = Math.max(index.truncatedBefore, dropped.at(-1).at)
        await this.collectBlobs(sessionId, index)
      }
      await this.save(sessionId, index)
      await this.pruneSessions(sessionId).catch((error) => this.warn(error))
    })
  }

  async collectBlobs(sessionId, index) {
    const used = new Set(index.snapshots.map((item) => item.blob).filter(Boolean))
    const dir = join(this.sessionDir(sessionId), 'blobs')
    for (const name of await readdir(dir).catch(() => [])) {
      if (!used.has(name)) await rm(join(dir, name), { force: true }).catch(() => {})
    }
  }

  // 淘汰最久没动过的会话快照，数据目录不无限增长；当前会话不淘汰。
  async pruneSessions(currentSessionId) {
    const dirs = await readdir(this.root, { withFileTypes: true }).catch(() => [])
    if (dirs.length <= MAX_CHECKPOINT_SESSIONS) return
    const keep = this.sessionDir(currentSessionId)
    const scored = []
    for (const dir of dirs) {
      if (!dir.isDirectory()) continue
      const path = join(this.root, dir.name)
      if (path === keep) continue
      const info = await stat(path).catch(() => null)
      if (info) scored.push({ path, mtime: info.mtimeMs })
    }
    scored.sort((left, right) => right.mtime - left.mtime)
    for (const stale of scored.slice(MAX_CHECKPOINT_SESSIONS - 1)) {
      await rm(stale.path, { recursive: true, force: true }).catch(() => {})
    }
  }

  // 每个文件在时间点 at 的内容来源：at 之后最早的那份快照。
  async plan(sessionId, at) {
    const index = await this.load(sessionId)
    const earliest = new Map()
    for (const snapshot of index.snapshots) {
      if (snapshot.at < at) continue
      const current = earliest.get(snapshot.file)
      if (!current || snapshot.at < current.at) earliest.set(snapshot.file, snapshot)
    }
    const files = []
    for (const snapshot of [...earliest.values()].sort((a, b) => a.file.localeCompare(b.file))) {
      files.push(await this.describe(sessionId, snapshot, index.written))
    }
    return {
      files,
      // 更早的快照被淘汰过，而目标时间落在淘汰范围内：结果可能不完整。
      incomplete: Boolean(index.truncatedBefore && at <= index.truncatedBefore),
    }
  }

  async describe(sessionId, snapshot, written = {}) {
    const path = relative(snapshot.cwd, snapshot.file).replaceAll('\\', '/')
    const base = { path, file: snapshot.file, cwd: snapshot.cwd }
    if (!nested(snapshot.cwd, snapshot.file) || !(await realInside(snapshot.cwd, snapshot.file)))
      return { ...base, action: 'unavailable', reason: 'path' }
    if (snapshot.reason) return { ...base, action: 'unavailable', reason: snapshot.reason }
    const current = await lstat(snapshot.file).catch(() => null)
    if (current && !current.isFile()) return { ...base, action: 'unavailable', reason: 'not-file' }
    // 本会话最后一次写下之后又被改过：回退会盖掉别人的修改，这个文件不动。
    if (Object.hasOwn(written, snapshot.file)) {
      const expected = written[snapshot.file]
      const now = current ? await fingerprint(snapshot.file) : null
      if (expected !== UNHASHABLE && now !== UNHASHABLE && now !== expected)
        return { ...base, action: 'unavailable', reason: 'changed' }
    }
    if (!snapshot.existed) return { ...base, action: current ? 'delete' : 'unchanged' }
    const blob = await readFile(this.blobPath(sessionId, snapshot.blob)).catch(() => null)
    if (!blob) return { ...base, action: 'unavailable', reason: 'missing' }
    if (current && current.size === blob.length) {
      const content = await readFile(snapshot.file).catch(() => null)
      if (content && content.equals(blob)) return { ...base, action: 'unchanged' }
    }
    return { ...base, action: 'restore', blob: snapshot.blob }
  }

  async preview(sessionId, at) {
    const { files, incomplete } = await this.plan(sessionId, at)
    return {
      files: files
        .filter((item) => item.action !== 'unchanged')
        .map(({ path, action, reason }) => ({ path, action, ...(reason ? { reason } : {}) })),
      incomplete,
    }
  }

  restore(sessionId, at) {
    return this.serialize(sessionId, async () => {
      const { files, incomplete } = await this.plan(sessionId, at)
      const result = { restored: [], deleted: [], unavailable: [], incomplete }
      // 回退写下的内容也算本会话写的：之后再回退不把它当成别人的修改。
      const written = {}
      for (const item of files) {
        if (item.action === 'restore') {
          const content = await readFile(this.blobPath(sessionId, item.blob))
          await mkdir(dirname(item.file), { recursive: true })
          await writeFile(item.file, content)
          written[item.file] = sha256(content)
          result.restored.push(item.path)
        } else if (item.action === 'delete') {
          await unlink(item.file)
          written[item.file] = null
          result.deleted.push(item.path)
        } else if (item.action === 'unavailable') {
          result.unavailable.push({ path: item.path, reason: item.reason })
        }
      }
      // 回退后文件回到了目标时间的样子：之后再改，按新的一轮重新记快照。
      const index = await this.load(sessionId)
      index.mark = Math.max(index.mark, this.now())
      Object.assign(index.written, written)
      await this.save(sessionId, index)
      return result
    })
  }

  async clear(sessionId) {
    this.indexes.delete(sessionId)
    await rm(this.sessionDir(sessionId), { recursive: true, force: true })
  }
}
