import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { resolveToCwd } from '../runtime/pi-coding-agent.mjs'
import {
  captureWorkspaceAssetBaseline,
  listChangedWorkspaceAssets,
  resolveWorkspaceAssetPath,
  workspaceAssetFile,
} from './workspace-asset-capture.mjs'
import { pisperError } from '../../shared/errors.mjs'

const FILE_TOOLS = new Set(['write', 'edit', 'bash', 'powershell'])
// shell 独占工作区的上限：多数命令在此之内结束，产物归属精确；更久的 watch、开发服务器
// 到点后让出锁，否则其它会话在同一项目里的命令和写入会一直排队。
const SHELL_EXCLUSIVE_MS = 5_000
// 一条命令改动的文件超过这个数，就是构建、代码生成、安装依赖或切换分支，不是 Agent 交付的结果：
// 整批不记。否则一次生成几千个类全进资产库，回复上挂几千个附件，资产索引也越写越大。
export const MAX_SHELL_CAPTURED_FILES = 500
// 待重试的产物超过这个数的会话，是旧版本整批记下的构建产物：读入时丢掉，不再在每轮收尾时重试。
export const MAX_PENDING_FILES_PER_SESSION = 1_000

function operation(name, args) {
  if (name === 'call_tool') {
    // 与网关的名称归一化保持一致，避免带空白的有效调用绕过捕获。
    name = String(args?.name || '').trim()
    args = args?.arguments
  }
  if (!FILE_TOOLS.has(name)) return null
  if (name === 'bash' || name === 'powershell') return { shell: true }
  return typeof args?.path === 'string' && args.path.trim() ? { path: args.path } : null
}

function nested(root, path) {
  const child = relative(root, path)
  return !isAbsolute(child) && child !== '..' && !child.startsWith(`..${sep}`)
}

async function canonical(path) {
  const resolved = await resolveWorkspaceAssetPath(path)
  return process.platform === 'win32' ? resolved.toLowerCase() : resolved
}

// 只串行化重叠工作区内的文件工具，模型推理和独立工作区仍可并发。
// 锁放在 execute 内，不能放 beforeToolCall：Pi 并行批次先准备全部调用才执行。
export class WorkspaceAssetTracker {
  constructor({
    dataDir,
    archive,
    // 一批归档结束后调用一次（写资产索引）；archive 本身只改内存。
    commit = async () => {},
    warn = (error) => console.warn('Asset archive will retry:', error.message),
    shellExclusiveMs = SHELL_EXCLUSIVE_MS,
  }) {
    this.dataDir = resolve(dataDir)
    this.shellExclusiveMs = shellExclusiveMs
    // 已让出锁、仍在运行的 shell；其它调用登记的产物记在这里，结束时不再重复认领。
    this.detached = new Set()
    this.path = join(dataDir, 'pisper-workspace-asset-retries.json')
    this.archive = archive
    this.commit = commit
    // 写资产索引在后台串行进行：命令结果不等它，本轮收尾的 drain 等它。
    this.committing = Promise.resolve()
    this.warn = warn
    this.pending = new Map()
    this.deliveries = new Map()
    this.installed = new WeakSet()
    this.wrapped = new WeakSet()
    this.waiters = []
    this.locks = new Set()
    this.saving = Promise.resolve()
    this.ready = Promise.all([canonical(dataDir), readJson(this.path, { files: [] })]).then(
      ([dataPath, state]) => {
        this.dataDir = dataPath
        const valid = (state.files || []).filter(
          (file) => typeof file?.sessionId === 'string' && typeof file?.path === 'string',
        )
        const counts = new Map()
        for (const file of valid) counts.set(file.sessionId, (counts.get(file.sessionId) || 0) + 1)
        for (const file of valid) {
          if (counts.get(file.sessionId) > MAX_PENDING_FILES_PER_SESSION) continue
          this.pending.set(this.key(file), file)
        }
        if (this.pending.size < valid.length) void this.save().catch((error) => this.warn(error))
      },
    )
  }

  key(file) {
    return JSON.stringify([file.sessionId, file.path])
  }

  save() {
    this.saving = this.saving
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, { files: [...this.pending.values()] }))
    return this.saving
  }

  pump() {
    for (const request of [...this.waiters]) {
      if (
        [...this.locks].some((lock) =>
          lock.scopes.some((a) => request.scopes.some((b) => nested(a, b) || nested(b, a))),
        )
      )
        continue
      this.waiters.splice(this.waiters.indexOf(request), 1)
      request.signal?.removeEventListener('abort', request.abort)
      this.locks.add(request)
      request.resolve(() => {
        this.locks.delete(request)
        this.pump()
      })
    }
  }

  async lock(scopes, signal) {
    const paths = await Promise.all(scopes.map(canonical))
    signal?.throwIfAborted()
    return new Promise((resolveLock, reject) => {
      const request = { scopes: paths, signal, resolve: resolveLock }
      request.abort = () => {
        const index = this.waiters.indexOf(request)
        if (index >= 0) this.waiters.splice(index, 1)
        reject(signal.reason || pisperError('cancelled', 'operation'))
      }
      signal?.addEventListener('abort', request.abort, { once: true })
      this.waiters.push(request)
      this.pump()
    })
  }

  install(session, { sessionId, cwd }) {
    if (!session?.agent || !cwd || this.installed.has(session)) return
    this.installed.add(session)
    const previous = session.agent.beforeToolCall
    session.agent.beforeToolCall = async (context, signal) => {
      // 先保留既有审批/文件范围检查；被拒绝的调用不会建立捕获任务。
      const decision = await previous?.(context, signal)
      if (decision?.block || signal?.aborted) return decision
      const name = context.toolCall.name
      if (!operation(name, context.args)) return decision
      const tool = context.context?.tools?.find((item) => item.name === name)
      if (tool && !this.wrapped.has(tool)) {
        this.wrapped.add(tool)
        const execute = tool.execute
        tool.execute = (...args) =>
          this.run({ sessionId, cwd, name, args: args[1], signal: args[2] }, () =>
            execute.apply(tool, args),
          )
      }
      return decision
    }
  }

  async run({ sessionId, cwd, name, args, signal }, execute) {
    const op = operation(name, args)
    if (!op) return execute()
    const path = op.path ? resolveToCwd(op.path, cwd) : null
    const scopes = path ? [cwd, dirname(path)] : [cwd]
    let release = await this.lock(scopes, signal)
    const unlock = () => {
      const done = release
      release = null
      done?.()
    }
    let detached = null
    try {
      signal?.throwIfAborted()
      await this.ready
      const before = op.shell
        ? await captureWorkspaceAssetBaseline(cwd, { exclude: [this.dataDir] }).catch(() => null)
        : await workspaceAssetFile(path)
      signal?.throwIfAborted()
      try {
        if (!op.shell) return await execute()
        const running = (async () => execute())()
        let timer
        const outlived = await Promise.race([
          running.then(
            () => false,
            () => false,
          ),
          new Promise((done) => {
            timer = setTimeout(done, this.shellExclusiveMs, true)
          }),
        ])
        clearTimeout(timer)
        if (outlived) {
          // 让出后其它调用可并发写入；它们登记过的同版本文件不算本命令的产物。
          detached = new Set()
          this.detached.add(detached)
          unlock()
        }
        return await running
      } finally {
        if (detached) this.detached.delete(detached)
        // shell 即使失败也可能已生成产物；显式 write/edit 则按实际路径记录同名更新。
        try {
          const internalPath = path && nested(this.dataDir, await canonical(path))
          let files = op.shell
            ? await listChangedWorkspaceAssets(before)
            : [await workspaceAssetFile(path)]
                .filter((file) => file && file.version !== before?.version && !internalPath)
                .map((file) => ({ ...file, added: !before }))
          if (op.shell && files.length > MAX_SHELL_CAPTURED_FILES) files = []
          if (detached || this.detached.size) {
            const claims = await Promise.all(files.map((file) => this.claim(file)))
            if (detached) files = files.filter((_, index) => !detached.has(claims[index]))
            for (const other of this.detached) for (const claim of claims) other.add(claim)
          }
          for (const file of files)
            this.pending.set(this.key({ ...file, sessionId }), { ...file, sessionId, cwd })
          if (files.length) {
            await this.save()
            // 归档本身只记内存，必须在让出锁之前做完：之后别的调用改了文件，版本就对不上了。
            const archived = await this.archiveFiles(
              files.map((file) => ({ ...file, sessionId, cwd })),
              { commit: false },
            )
            if (archived) this.commitInBackground()
          }
        } catch (error) {
          this.warn(error)
        }
      }
    } finally {
      unlock()
    }
  }

  // 排在前面的提交已经写进了之后的改动时，后面这次由 commit 自己判断为无事可做。
  commitInBackground() {
    this.committing = this.committing
      .catch(() => {})
      .then(() => this.commit())
      .catch((error) => this.warn(error))
  }

  async claim(file) {
    return JSON.stringify([await canonical(file.path), file.version])
  }

  // 返回是否归档了文件。commit=false 时由调用方在整批结束后自己提交。
  async archiveFiles(files, { save = true, commit = true } = {}) {
    let archived = false
    for (const file of files) {
      try {
        const current = await workspaceAssetFile(file.path)
        // 文件被另一轮覆盖后，不能把新内容归到旧轮次；新的写入由新调用另行登记。
        if (current?.version !== file.version) {
          this.pending.delete(this.key(file))
          continue
        }
        // 只有 Agent 新建的文件才挂到回复上；改动已有文件、更新代码只进资产。
        // 升级前落盘的重试项没有 added 字段，按原行为显示。
        const hiddenInReply = file.added === false
        const asset = await this.archive(file.sessionId, file.path, { hiddenInReply })
        if (!asset) continue
        archived = true
        this.pending.delete(this.key(file))
        if (hiddenInReply) continue
        const assets = this.deliveries.get(file.sessionId) || new Map()
        assets.set(asset.id, asset)
        this.deliveries.set(file.sessionId, assets)
      } catch (error) {
        this.warn(error)
      }
    }
    if (archived && commit) await this.commit()
    if (save) await this.save()
    return archived
  }

  async drain(sessionId) {
    await this.ready
    const files = [...this.pending.values()].filter((file) => file.sessionId === sessionId)
    let archived = false
    for (const file of files) {
      const release = await this.lock([file.cwd || dirname(file.path), dirname(file.path)])
      try {
        if (this.pending.get(this.key(file)) === file)
          archived = (await this.archiveFiles([file], { save: false, commit: false })) || archived
      } finally {
        release()
      }
    }
    // 每个文件都重写一遍资产索引和待重试清单是平方级的；整批结束时各写一次。
    if (archived) this.commitInBackground()
    await this.committing
    if (files.length) await this.save()
    const assets = [...(this.deliveries.get(sessionId)?.values() || [])]
    this.deliveries.delete(sessionId)
    return assets
  }
}
