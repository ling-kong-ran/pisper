// 执行环境：Bot 任务在哪台机器上跑命令。本机执行器（LocalExecutor）与执行节点（NodeExecutor，B6）
// 是同一个接口：worktree 管理只经 exec 调 git，工具经 operations 读写文件、跑命令。
// 这里只允许调用 git，参数一律是数组，不经过 shell。
import { execFile } from 'node:child_process'

const ALLOWED_FILES = new Set(['git'])
const DEFAULT_TIMEOUT_MS = 60_000
const MAX_OUTPUT_BYTES = 8 * 1024 * 1024

export class ExecutorError extends Error {
  constructor(code, message) {
    super(message)
    this.name = 'ExecutorError'
    this.code = code
    this.statusCode = 400
  }
}

export class LocalExecutor {
  constructor() {
    this.id = 'local'
    this.kind = 'local'
    this.capabilities = Object.freeze({ shell: true, git: true, browser: true })
  }

  online() {
    return true
  }

  // 运行一次命令；非零退出码照常返回（由调用方判断），只有启动失败或超时才抛错。
  exec(file, args, cwd, { signal, timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
    if (!ALLOWED_FILES.has(file))
      return Promise.reject(new ExecutorError('command_not_allowed', `不允许执行 ${file}。`))
    if (!Array.isArray(args) || args.some((arg) => typeof arg !== 'string' || arg.includes('\0')))
      return Promise.reject(new ExecutorError('command_invalid', '命令参数无效。'))
    return new Promise((resolve, reject) => {
      execFile(
        file,
        args,
        {
          cwd,
          signal,
          timeout: timeoutMs,
          maxBuffer: MAX_OUTPUT_BYTES,
          windowsHide: true,
          // 不让 git 停下来等凭据或编辑器输入：Bot 任务没有人在终端前。
          env: { ...process.env, GIT_TERMINAL_PROMPT: '0', GIT_EDITOR: 'true' },
        },
        (error, stdout, stderr) => {
          if (error && typeof error.code !== 'number') {
            reject(new ExecutorError('command_failed', `${file} 执行失败：${error.message}`))
            return
          }
          resolve({ code: error ? error.code : 0, stdout: String(stdout), stderr: String(stderr) })
        },
      )
    })
  }
}

// 另一台电脑上的执行节点（B6）：git 与工具的文件、命令操作经连接中枢转发过去，由节点在它允许的目录里执行。
// operations() 的形状就是引擎 read / edit / write / ls / find / bash 工具可替换的 operations 接口。
// grep 依赖本机 ripgrep，引擎没有远程接口，远程会话里不提供（模型可以用 bash 的 grep）。
export class NodeExecutor {
  constructor({ hub, environment }) {
    this.hub = hub
    this.id = environment.id
    this.kind = 'node'
    this.name = environment.name
    this.capabilities = Object.freeze({
      shell: environment.capabilities?.shell === true,
      git: environment.capabilities?.git !== false,
      browser: false,
    })
  }

  online() {
    return this.hub.online(this.id)
  }

  #call(op, args, options) {
    return this.hub.call(this.id, op, args, options)
  }

  exec(file, args, cwd, { signal, timeoutMs = DEFAULT_TIMEOUT_MS } = {}) {
    if (!ALLOWED_FILES.has(file))
      return Promise.reject(new ExecutorError('command_not_allowed', `不允许执行 ${file}。`))
    if (!Array.isArray(args) || args.some((arg) => typeof arg !== 'string' || arg.includes('\0')))
      return Promise.reject(new ExecutorError('command_invalid', '命令参数无效。'))
    return this.#call(
      'exec',
      { file, args, cwd, timeoutMs },
      { signal, timeoutMs: timeoutMs + 30_000 },
    ).then((value) => ({
      code: Number.isInteger(value?.code) ? value.code : 1,
      stdout: String(value?.stdout ?? ''),
      stderr: String(value?.stderr ?? ''),
    }))
  }

  operations() {
    const readFile = async (path) =>
      Buffer.from(String((await this.#call('fs.read', { path })) ?? ''), 'base64')
    const access = async (path, mode) => {
      await this.#call('fs.access', { path, mode })
    }
    const writeFile = async (path, content) => {
      await this.#call('fs.write', { path, content: String(content) })
    }
    const stat = async (path) => {
      const value = await this.#call('fs.stat', { path })
      if (!value?.exists) throw new ExecutorError('not_found', `找不到 ${path}`)
      const directory = value.isDirectory === true
      return { isDirectory: () => directory }
    }
    const exists = async (path) => Boolean((await this.#call('fs.stat', { path }))?.exists)
    return {
      read: { readFile, access: (path) => access(path, 'r') },
      edit: { readFile, writeFile, access: (path) => access(path, 'rw') },
      write: {
        writeFile,
        mkdir: async (path) => {
          await this.#call('fs.mkdir', { path })
        },
      },
      ls: {
        exists,
        stat,
        readdir: async (path) => {
          const names = await this.#call('fs.readdir', { path })
          return Array.isArray(names) ? names.map(String) : []
        },
      },
      find: {
        exists,
        glob: async (pattern, cwd, { ignore = [], limit = 1000 } = {}) => {
          const paths = await this.#call('fs.glob', { pattern, cwd, ignore, limit })
          return Array.isArray(paths) ? paths.map(String) : []
        },
      },
      bash: {
        exec: async (command, cwd, { onData, signal, timeout } = {}) => {
          if (!this.capabilities.shell)
            throw new ExecutorError('shell_disabled', '这台电脑没有允许执行命令。')
          const timeoutMs = timeout ? Math.max(1, Number(timeout)) * 1000 : 10 * 60_000
          const value = await this.#call(
            'shell',
            { command: String(command), cwd, timeoutMs },
            { signal, timeoutMs: timeoutMs + 30_000, onChunk: onData },
          )
          return { exitCode: Number.isInteger(value?.exitCode) ? value.exitCode : null }
        },
      },
    }
  }
}
