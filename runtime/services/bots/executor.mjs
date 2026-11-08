// 执行节点（B6）：Bot 的电脑是另一台机器时，会话的文件与命令工具经连接中枢转发过去，
// 由节点在它允许的目录里执行。本机的 Bot 直接用本机工具，不经过这里。

// 执行节点的错误保留各自的 code（界面与 SSH 机器按它分辨离线、版本太旧、目录不允许等），message 用英文。
export class ExecutorError extends Error {
  constructor(code, message) {
    super(message)
    this.name = 'ExecutorError'
    this.code = code
    this.statusCode = 400
  }
}

// operations() 的形状就是引擎 read / edit / write / ls / find / bash 工具可替换的 operations 接口。
// grep 依赖本机 ripgrep，引擎没有远程接口，远程会话里不提供（模型可以用 bash 的 grep）。
export class NodeExecutor {
  constructor({ hub, environment }) {
    this.hub = hub
    this.id = environment.id
    this.kind = 'node'
    this.name = environment.name
    this.roots = Array.isArray(environment.roots) ? environment.roots.map(String) : []
    this.capabilities = Object.freeze({
      shell: environment.capabilities?.shell === true,
      browser: false,
    })
  }

  online() {
    return this.hub.online(this.id)
  }

  #call(op, args, options) {
    return this.hub.call(this.id, op, args, options)
  }

  // 列出那台电脑上一层子文件夹，供界面点选 Bot 的目录；不给路径时从它允许的第一个目录开始。
  // 旧版节点没有这个操作，明确回“版本太旧”，不拿空列表冒充。
  async listFolders(path, { signal } = {}) {
    const target = String(path || '').trim() || this.roots[0] || ''
    if (!target)
      throw new ExecutorError(
        'path_not_allowed',
        'Folder is not allowed: this machine allows none.',
      )
    let value
    try {
      value = await this.#call('fs.folders', { path: target }, { signal })
    } catch (error) {
      if (error?.code === 'op_unsupported')
        throw new ExecutorError(
          'executor_outdated',
          'Machine is not supported here: update pisper on it to browse folders, or type the path',
        )
      throw error
    }
    const record = value && typeof value === 'object' ? value : {}
    const text = (field) => (typeof field === 'string' ? field : '')
    return {
      path: text(record.path),
      parent: text(record.parent),
      folders: (Array.isArray(record.folders) ? record.folders : []).flatMap((folder) =>
        folder && typeof folder.name === 'string' && typeof folder.path === 'string'
          ? [{ name: folder.name, path: folder.path }]
          : [],
      ),
      truncated: record.truncated === true,
    }
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
      if (!value?.exists)
        throw new ExecutorError('not_found', `ENOENT: no such file or directory, '${path}'`)
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
            throw new ExecutorError(
              'shell_disabled',
              'Machine does not allow commands (--no-shell).',
            )
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
