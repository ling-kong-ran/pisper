// 执行环境（B2）：Bot 任务在哪台机器上跑命令。B2 只有本机执行器；执行节点（NodeExecutor）在 B6 接入，
// 届时通过同一个接口把 git 与工具的文件、命令操作转发到另一台机器。
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
