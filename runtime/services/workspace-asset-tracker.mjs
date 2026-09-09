import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { resolveToCwd } from '../runtime/pi-coding-agent.mjs'
import {
  captureWorkspaceAssetBaseline,
  listChangedWorkspaceAssets,
  resolveWorkspaceAssetPath,
  workspaceAssetFile,
} from './workspace-asset-capture.mjs'

const FILE_TOOLS = new Set(['write', 'edit', 'bash', 'powershell'])

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
    warn = (error) => console.warn('资产归档待重试：', error.message),
  }) {
    this.dataDir = resolve(dataDir)
    this.path = join(dataDir, 'pisper-workspace-asset-retries.json')
    this.archive = archive
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
        for (const file of state.files || []) {
          if (typeof file?.sessionId === 'string' && typeof file?.path === 'string')
            this.pending.set(this.key(file), file)
        }
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
        reject(signal.reason || new Error('操作已停止。'))
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
    const release = await this.lock(scopes, signal)
    try {
      signal?.throwIfAborted()
      await this.ready
      const before = op.shell
        ? await captureWorkspaceAssetBaseline(cwd, { exclude: [this.dataDir] }).catch(() => null)
        : await workspaceAssetFile(path)
      signal?.throwIfAborted()
      try {
        return await execute()
      } finally {
        // shell 即使失败也可能已生成产物；显式 write/edit 则按实际路径记录同名更新。
        try {
          const internalPath = path && nested(this.dataDir, await canonical(path))
          const files = op.shell
            ? await listChangedWorkspaceAssets(before)
            : [await workspaceAssetFile(path)].filter(
                (file) => file && file.version !== before?.version && !internalPath,
              )
          for (const file of files)
            this.pending.set(this.key({ ...file, sessionId }), { ...file, sessionId, cwd })
          if (files.length) {
            await this.save()
            await this.archiveFiles(files.map((file) => ({ ...file, sessionId, cwd })))
          }
        } catch (error) {
          this.warn(error)
        }
      }
    } finally {
      release()
    }
  }

  async archiveFiles(files) {
    for (const file of files) {
      try {
        const current = await workspaceAssetFile(file.path)
        // 文件被另一轮覆盖后，不能把新内容归到旧轮次；新的写入由新调用另行登记。
        if (current?.version !== file.version) {
          this.pending.delete(this.key(file))
          continue
        }
        const asset = await this.archive(file.sessionId, file.path)
        if (!asset) continue
        this.pending.delete(this.key(file))
        const assets = this.deliveries.get(file.sessionId) || new Map()
        assets.set(asset.id, asset)
        this.deliveries.set(file.sessionId, assets)
      } catch (error) {
        this.warn(error)
      }
    }
    await this.save()
  }

  async drain(sessionId) {
    await this.ready
    const files = [...this.pending.values()].filter((file) => file.sessionId === sessionId)
    for (const file of files) {
      const release = await this.lock([file.cwd || dirname(file.path), dirname(file.path)])
      try {
        if (this.pending.get(this.key(file)) === file) await this.archiveFiles([file])
      } finally {
        release()
      }
    }
    const assets = [...(this.deliveries.get(sessionId)?.values() || [])]
    this.deliveries.delete(sessionId)
    return assets
  }
}
