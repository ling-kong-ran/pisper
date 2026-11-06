// SSH 机器的执行器：会话的文件与命令工具经系统自带的 ssh 客户端在对方机器上执行，对方不用装 Pisper。
// 接口和执行节点一样（operations() 就是引擎工具可替换的 operations）。用系统 ssh 而不是 JS 实现：
// ~/.ssh/config、密钥、ssh-agent、ProxyJump 全部照旧，Pisper 不碰也不保存任何凭据。
// 对方只要是有 POSIX shell 的 SSH 服务就行（OpenSSH、Dropbear、Tailscale SSH）；不依赖 SFTP，也不要 PTY。
import { spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { mkdirSync } from 'node:fs'
import { posix } from 'node:path'
import { ExecutorError } from './bots/executor.mjs'

const CONNECT_TIMEOUT_SECONDS = 10
const OPERATION_TIMEOUT_MS = 60_000
const PROBE_TIMEOUT_MS = 25_000
const KILL_TIMEOUT_MS = 8_000
const MAX_READ_BYTES = 16 * 1024 * 1024
const MAX_STDERR_CHARS = 4_000
const MAX_GLOB_ENTRIES = 200_000
const MAX_GLOB_RESULTS = 10_000
const LIST_CACHE_TTL_MS = 5_000
// ssh 退出后 stdout 还没结束时最多再等这么久：ControlPersist 分出的后台进程可能还握着管道。
const EXIT_DRAIN_MS = 1_000
// Unix socket 路径上限（macOS 104 字节）：%C 展开后是 40 位十六进制。
const MAX_MUX_DIRECTORY_CHARS = 60

const EXIT_MISSING = 44
const EXIT_IS_DIRECTORY = 45
const EXIT_DENIED = 46
const EXIT_NO_CWD = 97
// ssh 客户端自己失败（连不上、认证失败）的退出码；远程命令的退出码由对方决定。
const EXIT_SSH_FAILED = 255
const PROBE_MARKER = 'PISPER_SSH_PROBE'
const REQUIRED_COMMANDS = ['cat', 'find', 'ls', 'mkdir']

// 远程命令总要经对方的 shell 解析一次：路径和命令一律单引号包起来，没有拼接的余地。
export function shellQuote(value) {
  const text = String(value)
  if (text.includes('\0')) throw new ExecutorError('invalid_path', '路径不能包含空字符。')
  return `'${text.replaceAll("'", `'\\''`)}'`
}

// 大脑在 Windows 时引擎工具用 path.resolve 拼路径，会把 /srv/app/a 变成 C:\srv\app\a。
// 对方一定是 POSIX，这里还原回去。
export function toRemotePath(value, platform = process.platform) {
  const text = String(value)
  if (platform !== 'win32') return text
  return text.replace(/^[A-Za-z]:(?=[\\/])/, '').replaceAll('\\', '/')
}

// 选项写死在这里：不转发 agent 和端口（对方机器不该能用你的密钥），首次连接记下指纹，之后指纹变了就拒绝。
export function sshArguments(machine, { muxDirectory = '', extraArguments = [] } = {}) {
  return [
    '-T',
    '-o',
    'BatchMode=yes',
    '-o',
    `ConnectTimeout=${CONNECT_TIMEOUT_SECONDS}`,
    '-o',
    'ServerAliveInterval=15',
    '-o',
    'ServerAliveCountMax=3',
    '-o',
    'StrictHostKeyChecking=accept-new',
    '-o',
    'ForwardAgent=no',
    '-o',
    'ForwardX11=no',
    '-o',
    'ClearAllForwardings=yes',
    '-o',
    'LogLevel=ERROR',
    ...(muxDirectory
      ? [
          '-o',
          'ControlMaster=auto',
          '-o',
          `ControlPath=${muxDirectory}/%C`,
          '-o',
          'ControlPersist=120',
        ]
      : []),
    ...(machine.port ? ['-p', String(machine.port)] : []),
    ...extraArguments,
    '--',
    machine.target,
  ]
}

// ssh 客户端的错误只有英文，分类后由界面按语言给出下一步。
export function classifySshFailure(stderr) {
  const text = String(stderr)
  if (/REMOTE HOST IDENTIFICATION HAS CHANGED|Host key verification failed/i.test(text))
    return 'host_key_changed'
  if (/Connection refused/i.test(text)) return 'connection_refused'
  if (/Permission denied|Too many authentication failures|publickey/i.test(text))
    return 'auth_failed'
  if (
    /timed out|No route to host|Network is unreachable|Could not resolve hostname|Name or service not known|nodename nor servname|Temporary failure in name resolution/i.test(
      text,
    )
  )
    return 'unreachable'
  return 'connection_failed'
}

const FAILURE_MESSAGES = {
  ssh_client_missing: '这台电脑上找不到 ssh 命令。',
  host_key_changed:
    '对方的主机指纹变了，为了安全已拒绝连接。确认机器确实重装过之后，再更新 known_hosts。',
  connection_refused: '连接被拒绝：对方可能没有运行 SSH 服务，或者端口不对。',
  auth_failed: '对方拒绝了登录：Pisper 只用密钥登录，请先把密钥配到对方机器上。',
  unreachable: '连不上对方机器：请检查地址、网络和防火墙。',
  connection_failed: '和对方机器的 SSH 连接失败。',
}

function sshFailure(code) {
  return new ExecutorError(code, FAILURE_MESSAGES[code] || FAILURE_MESSAGES.connection_failed)
}

function collectStderr(child) {
  let stderr = ''
  child.stderr?.on('data', (chunk) => {
    if (stderr.length < MAX_STDERR_CHARS) stderr += chunk
  })
  return () => stderr.slice(0, MAX_STDERR_CHARS)
}

// ssh 退出后 stdout 可能还有没读完的数据，但不能等 close：连接复用时分出的后台进程可能一直握着管道。
// 结束时主动放开两条管道，不留下要等它退出才释放的句柄。
function whenFinished(child, done) {
  let exitCode = null
  let exited = false
  let stdoutEnded = false
  let finished = false
  let drainTimer
  const finish = () => {
    if (finished) return
    finished = true
    clearTimeout(drainTimer)
    child.stdout?.destroy()
    child.stderr?.destroy()
    done(exitCode)
  }
  child.stdout?.once('end', () => {
    stdoutEnded = true
    if (exited) finish()
  })
  child.once('exit', (code, signal) => {
    exited = true
    exitCode = code ?? (signal ? 128 : 1)
    if (stdoutEnded) finish()
    else drainTimer = setTimeout(finish, EXIT_DRAIN_MS)
  })
  child.once('close', () => {
    if (exited) finish()
  })
}

// 在 glob 里和执行节点同一套语义（packages/pisper/lib/executor-node.mjs）：npm 包不依赖 runtime，这里各留一份。
function globToRegExp(pattern) {
  let source = ''
  const text = String(pattern).replaceAll('\\', '/')
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index]
    if (char === '*') {
      if (text[index + 1] === '*') {
        const slash = text[index + 2] === '/'
        source += slash ? '(?:.*/)?' : '.*'
        index += slash ? 2 : 1
      } else source += '[^/]*'
    } else if (char === '?') source += '[^/]'
    else if (char === '{') {
      const end = text.indexOf('}', index)
      if (end < 0) source += '\\{'
      else {
        source += `(?:${text
          .slice(index + 1, end)
          .split(',')
          .map((part) => part.replace(/[.+^$()|[\]\\]/g, '\\$&').replaceAll('*', '[^/]*'))
          .join('|')})`
        index = end
      }
    } else source += char.replace(/[.+^$()|[\]\\]/g, '\\$&')
  }
  return new RegExp(`^${source}$`)
}

export class SshExecutor {
  constructor({
    machine,
    muxDirectory = '',
    extraArguments = [],
    platform = process.platform,
    spawnProcess = spawn,
    now = () => Date.now(),
    isAvailable = () => true,
  }) {
    this.machine = machine
    this.id = machine.id
    this.kind = 'ssh'
    this.name = machine.name
    this.capabilities = Object.freeze({ shell: true, browser: false })
    this.platform = platform
    this.spawnProcess = spawnProcess
    this.now = now
    this.extraArguments = extraArguments
    // 连接复用：Windows 版 OpenSSH 不支持；路径太长时 ssh 会直接报错，宁可每次新连。
    this.muxDirectory =
      platform !== 'win32' && muxDirectory && muxDirectory.length <= MAX_MUX_DIRECTORY_CHARS
        ? muxDirectory
        : ''
    this.listCache = new Map()
    this.isAvailable = isAvailable
  }

  online() {
    return this.machine.lastCheck?.ok === true
  }

  #arguments() {
    if (this.muxDirectory) mkdirSync(this.muxDirectory, { recursive: true, mode: 0o700 })
    return sshArguments(this.machine, {
      muxDirectory: this.muxDirectory,
      extraArguments: this.extraArguments,
    })
  }

  #spawn(remoteCommand) {
    // 驻留的会话还拿着这个执行器：机器移除后不能再连过去。
    if (!this.isAvailable()) throw new ExecutorError('machine_not_found', '这台机器已被移除。')
    return this.spawnProcess('ssh', [...this.#arguments(), remoteCommand], {
      stdio: ['pipe', 'pipe', 'pipe'],
      windowsHide: true,
    })
  }

  // 跑一段短脚本，结果整段收回来。maxBytes 防止把巨大的文件读进内存。
  #run(script, { stdin, timeoutMs = OPERATION_TIMEOUT_MS, maxBytes = 0 } = {}) {
    return new Promise((resolve, reject) => {
      let child
      try {
        child = this.#spawn(`sh -c ${shellQuote(script)}`)
      } catch (error) {
        reject(error)
        return
      }
      const chunks = []
      let size = 0
      let overflow = false
      let settled = false
      const stderr = collectStderr(child)
      const timer = setTimeout(() => {
        finish(new ExecutorError('ssh_timeout', '对方机器太久没有响应。'))
        child.kill()
      }, timeoutMs)
      const finish = (error, exitCode) => {
        if (settled) return
        settled = true
        clearTimeout(timer)
        if (error) {
          child.stdout?.destroy()
          child.stderr?.destroy()
          reject(error)
          return
        }
        if (exitCode === EXIT_SSH_FAILED) {
          reject(sshFailure(classifySshFailure(stderr())))
          return
        }
        resolve({ code: exitCode, stdout: Buffer.concat(chunks), stderr: stderr(), overflow })
      }
      child.on('error', (error) =>
        finish(error?.code === 'ENOENT' ? sshFailure('ssh_client_missing') : error),
      )
      child.stdout.on('data', (chunk) => {
        size += chunk.length
        if (maxBytes && size > maxBytes) {
          overflow = true
          child.kill()
          return
        }
        chunks.push(chunk)
      })
      whenFinished(child, (exitCode) => finish(null, exitCode))
      child.stdin.on('error', () => {})
      child.stdin.end(stdin ?? undefined)
    })
  }

  #path(value) {
    return shellQuote(toRemotePath(value, this.platform))
  }

  #fail(result, path, what) {
    const shown = toRemotePath(path, this.platform)
    if (result.code === EXIT_MISSING)
      throw new ExecutorError('not_found', `ENOENT: no such file or directory, ${what} '${shown}'`)
    if (result.code === EXIT_IS_DIRECTORY)
      throw new ExecutorError(
        'is_directory',
        `EISDIR: illegal operation on a directory, ${what} '${shown}'`,
      )
    if (result.code === EXIT_DENIED)
      throw new ExecutorError('permission_denied', `EACCES: permission denied, ${what} '${shown}'`)
    const detail = result.stderr.trim().slice(0, 300)
    throw new ExecutorError('remote_failed', detail || `${what} '${shown}' 失败。`)
  }

  // 真实连上去跑一遍，不看对方装的是哪种 SSH 服务：有 shell、命令齐全才算能用。
  async probe() {
    const script = [
      `printf '%s\\n' ${PROBE_MARKER}`,
      'uname -s 2>/dev/null || echo unknown',
      `printf '%s\\n' "\${HOME:-}"`,
      `for name in ${REQUIRED_COMMANDS.join(' ')}; do command -v "$name" >/dev/null 2>&1 || printf 'missing:%s\\n' "$name"; done`,
    ].join('; ')
    const result = await this.#run(script, { timeoutMs: PROBE_TIMEOUT_MS })
    const lines = result.stdout.toString('utf8').split(/\r?\n/)
    const start = lines.indexOf(PROBE_MARKER)
    if (start < 0)
      throw new ExecutorError(
        'unsupported_remote',
        '对方没有可用的 POSIX shell（Windows 的 SSH 服务和受限账号暂不支持）。',
      )
    const missing = lines
      .filter((line) => line.startsWith('missing:'))
      .map((line) => line.slice('missing:'.length))
    if (missing.length)
      throw new ExecutorError('unsupported_remote', `对方缺少这些命令：${missing.join('、')}。`)
    return { platform: lines[start + 1] || 'unknown', home: lines[start + 2] || '' }
  }

  // 目录解析成绝对路径：~ 和空值都从对方的 HOME 算起。目录不存在时明确报错，不等到第一个工具才失败。
  async resolveDirectory(input) {
    const text = String(input ?? '').trim()
    const target =
      !text || text === '~'
        ? '"$HOME"'
        : text.startsWith('~/')
          ? `"$HOME"/${shellQuote(text.slice(2))}`
          : this.#path(text)
    const result = await this.#run(`cd ${target} 2>/dev/null || exit ${EXIT_MISSING}; pwd`)
    if (result.code === EXIT_MISSING)
      throw new ExecutorError('machine_folder_missing', `对方机器上没有这个目录：${text || '~'}`)
    if (result.code !== 0) this.#fail(result, text, 'cd')
    return result.stdout.toString('utf8').trim()
  }

  async #readFile(path) {
    const quoted = this.#path(path)
    const result = await this.#run(
      `[ -e ${quoted} ] || exit ${EXIT_MISSING}; [ -d ${quoted} ] && exit ${EXIT_IS_DIRECTORY}; exec cat -- ${quoted}`,
      { maxBytes: MAX_READ_BYTES },
    )
    if (result.overflow) throw new ExecutorError('too_large', '文件太大，读不了。')
    if (result.code !== 0) this.#fail(result, path, 'open')
    return result.stdout
  }

  async #access(path, mode) {
    const quoted = this.#path(path)
    const checks = [
      `[ -e ${quoted} ] || exit ${EXIT_MISSING}`,
      `[ -r ${quoted} ] || exit ${EXIT_DENIED}`,
    ]
    if (mode === 'rw') checks.push(`[ -w ${quoted} ] || exit ${EXIT_DENIED}`)
    const result = await this.#run(checks.join('; '))
    if (result.code !== 0) this.#fail(result, path, 'access')
  }

  async #writeFile(path, content) {
    this.listCache.clear()
    const result = await this.#run(`cat > ${this.#path(path)}`, { stdin: String(content) })
    if (result.code !== 0) this.#fail(result, path, 'open')
  }

  async #mkdir(path) {
    this.listCache.clear()
    const result = await this.#run(`mkdir -p -- ${this.#path(path)}`)
    if (result.code !== 0) this.#fail(result, path, 'mkdir')
  }

  // 引擎的 ls 工具对每一项都查一次 stat：列目录时顺带记下类型，免得几百次连接。
  #cached(path) {
    const entry = this.listCache.get(path)
    if (!entry) return null
    if (this.now() - entry.at > LIST_CACHE_TTL_MS) {
      this.listCache.delete(path)
      return null
    }
    return entry
  }

  async #kind(path) {
    const remote = toRemotePath(path, this.platform)
    const cached = this.#cached(remote)
    if (cached) return cached.directory ? 'directory' : 'file'
    const quoted = shellQuote(remote)
    const result = await this.#run(
      `if [ -d ${quoted} ]; then echo directory; elif [ -e ${quoted} ]; then echo file; else echo none; fi`,
    )
    if (result.code !== 0) this.#fail(result, path, 'stat')
    return result.stdout.toString('utf8').trim()
  }

  async #readdir(path) {
    const remote = toRemotePath(path, this.platform)
    const quoted = shellQuote(remote)
    const result = await this.#run(
      `[ -d ${quoted} ] || exit ${EXIT_MISSING}; cd ${quoted} || exit ${EXIT_DENIED}; LC_ALL=C exec ls -Ap1`,
    )
    if (result.code !== 0) this.#fail(result, path, 'scandir')
    const at = this.now()
    return result.stdout
      .toString('utf8')
      .split('\n')
      .filter(Boolean)
      .map((line) => {
        const directory = line.endsWith('/')
        const name = directory ? line.slice(0, -1) : line
        this.listCache.set(posix.join(remote, name), { directory, at })
        return name
      })
  }

  async #glob(pattern, cwd, { ignore = [], limit = 1000 } = {}) {
    const matcher = globToRegExp(pattern)
    const ignored = (Array.isArray(ignore) ? ignore : []).map(globToRegExp)
    const max = Math.min(Math.max(1, Number(limit) || 1000), MAX_GLOB_RESULTS)
    // 依赖目录动辄几十万个文件：除非明确在找它，否则不进去。
    const prune = String(pattern).includes('node_modules')
      ? '-name .git'
      : '\\( -name .git -o -name node_modules \\)'
    const result = await this.#run(
      `cd ${this.#path(cwd)} || exit ${EXIT_MISSING}; find . ${prune} -prune -o -print 2>/dev/null | head -n ${MAX_GLOB_ENTRIES}`,
    )
    if (result.code === EXIT_MISSING) this.#fail(result, cwd, 'scandir')
    const found = []
    for (const line of result.stdout.toString('utf8').split('\n')) {
      if (found.length >= max) break
      if (!line || line === '.') continue
      const relative = line.replace(/^\.\//, '')
      if (ignored.some((rule) => rule.test(relative) || rule.test(`${relative}/`))) continue
      if (matcher.test(relative)) found.push(relative)
    }
    return found
  }

  // 结束对方的整个进程组：断开 ssh 不会结束没有终端的远程进程。
  #killRemote(pid) {
    const script = [
      `pg=$(cut -d' ' -f5 /proc/${pid}/stat 2>/dev/null)`,
      `[ -n "$pg" ] || pg=$(ps -o pgid= -p ${pid} 2>/dev/null | tr -d ' ')`,
      `if [ -n "$pg" ] && [ "$pg" != 0 ] && [ "$pg" != 1 ]; then kill -s TERM -- "-$pg" 2>/dev/null; else kill -s TERM -- ${pid} 2>/dev/null; fi`,
    ].join('; ')
    return this.#run(script, { timeoutMs: KILL_TIMEOUT_MS }).catch(() => {})
  }

  // 没有 PTY：靠带随机串的标记知道命令何时结束和它的退出码。命令一结束就断开，
  // 留在后台的进程（npm run dev &）继续跑，也不会因为它还握着管道而把工具卡住，和本机行为一致。
  // 标记不给模型看；stdin 接 /dev/null，sudo 这类要输入的命令立刻失败，不会一直挂着。
  #exec(command, cwd, { onData, signal, timeout } = {}) {
    return new Promise((resolve, reject) => {
      if (signal?.aborted) {
        reject(new Error('aborted'))
        return
      }
      const nonce = randomBytes(8).toString('hex')
      const separator = Buffer.from('\u001e', 'utf8')
      const prefix = Buffer.from(`\u001e${nonce}`, 'utf8')
      const script =
        `printf '\\036${nonce}P%s\\036' "$$"; ` +
        `cd ${this.#path(cwd)} 2>/dev/null || exit ${EXIT_NO_CWD}; ` +
        `"\${SHELL:-/bin/sh}" -lc ${shellQuote(command)} </dev/null 2>&1; rc=$?; ` +
        `printf '\\036${nonce}E%s\\036' "$rc"`
      let child
      try {
        child = this.#spawn(`sh -c ${shellQuote(script)}`)
      } catch (error) {
        reject(error)
        return
      }
      const stderr = collectStderr(child)
      let pending = Buffer.alloc(0)
      let remotePid = 0
      let markerExit = null
      let settled = false
      let stopping = ''
      let timer
      const cleanup = () => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', onAbort)
      }
      const finish = (error, value) => {
        if (settled) return
        settled = true
        cleanup()
        child.stdout?.destroy()
        child.stderr?.destroy()
        if (error) reject(error)
        else resolve(value)
      }
      const stop = (reason) => {
        if (stopping || settled) return
        stopping = reason
        const killed = remotePid ? this.#killRemote(remotePid) : Promise.resolve()
        void killed.finally(() => {
          child.kill()
          finish(new Error(reason === 'timeout' ? `timeout:${timeout}` : 'aborted'))
        })
      }
      const onAbort = () => stop('abort')
      // 标记可能被切在两个数据块之间：留够一个标记长度不发出去，下一块来了再看。
      const consume = (chunk) => {
        pending = pending.length ? Buffer.concat([pending, chunk]) : chunk
        for (;;) {
          const start = pending.indexOf(prefix)
          if (start < 0) break
          const end = pending.indexOf(separator, start + prefix.length)
          if (end < 0) break
          if (start > 0) onData?.(pending.subarray(0, start))
          const body = pending.subarray(start + prefix.length, end).toString('utf8')
          pending = pending.subarray(end + separator.length)
          if (body[0] === 'P') remotePid = Number(body.slice(1)) || 0
          else if (body[0] === 'E') {
            markerExit = Number(body.slice(1))
            child.kill()
            finish(null, { exitCode: Number.isInteger(markerExit) ? markerExit : null })
            return
          }
        }
        const hold = Math.min(pending.length, prefix.length + 24)
        const ready = pending.length - hold
        if (ready > 0) {
          onData?.(pending.subarray(0, ready))
          pending = pending.subarray(ready)
        }
      }
      if (timeout) timer = setTimeout(() => stop('timeout'), Math.max(1, Number(timeout)) * 1000)
      signal?.addEventListener('abort', onAbort, { once: true })
      child.on('error', (error) =>
        finish(error?.code === 'ENOENT' ? sshFailure('ssh_client_missing') : error),
      )
      child.stdout.on('data', consume)
      whenFinished(child, (code) => {
        if (settled || stopping) return
        if (pending.length) onData?.(pending)
        if (code === EXIT_NO_CWD)
          finish(new Error(`Working directory does not exist: ${toRemotePath(cwd, this.platform)}`))
        else if (code === EXIT_SSH_FAILED) finish(sshFailure(classifySshFailure(stderr())))
        // 没等到结束标记就断了：命令是否跑完不知道，不能当成成功。
        else
          finish(
            new ExecutorError('ssh_disconnected', '和对方机器的连接断开了，命令可能没有跑完。'),
          )
      })
      child.stdin.on('error', () => {})
      child.stdin.end()
    })
  }

  operations() {
    const readFile = (path) => this.#readFile(path)
    const stat = async (path) => {
      const kind = await this.#kind(path)
      if (kind === 'none')
        throw new ExecutorError('not_found', `ENOENT: no such file or directory, stat '${path}'`)
      return { isDirectory: () => kind === 'directory' }
    }
    const exists = async (path) => (await this.#kind(path)) !== 'none'
    return {
      read: { readFile, access: (path) => this.#access(path, 'r') },
      edit: {
        readFile,
        writeFile: (path, content) => this.#writeFile(path, content),
        access: (path) => this.#access(path, 'rw'),
      },
      write: {
        writeFile: (path, content) => this.#writeFile(path, content),
        mkdir: (path) => this.#mkdir(path),
      },
      ls: { exists, stat, readdir: (path) => this.#readdir(path) },
      find: { exists, glob: (pattern, cwd, options) => this.#glob(pattern, cwd, options) },
      bash: { exec: (command, cwd, options) => this.#exec(command, cwd, options) },
    }
  }
}
