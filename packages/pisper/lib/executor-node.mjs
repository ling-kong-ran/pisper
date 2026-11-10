// Pisper 执行节点（B6）：让另一台电脑上的 Pisper（“大脑”）在这台电脑上替 Bot 读写代码、跑命令。
//   pisper executor pair --brain https://<地址>:<端口> --fingerprint SHA256:… --code ABCD-EF23 --root <目录> [--root …] [--name 名字] [--no-shell]
//   pisper executor            # 连上大脑，开始干活（Ctrl+C 停止）
// 设计要点（runtime/services/bots/README.md「执行环境接口」）：
// - 节点主动连回大脑（HTTPS 长轮询），不开放端口；只认配对时确认过的证书指纹。
// - 节点不保存会话、不持有模型密钥；只保存配对信息（0600）。
// - 文件操作只在 --root 允许的目录里，按真实路径检查（符号链接绕不出去）；--no-shell 时不执行任意命令。
// 只用 Node 内置模块：npm 包里的这个文件不依赖下载的应用本体。
import { spawn } from 'node:child_process'
import { X509Certificate } from 'node:crypto'
import { existsSync } from 'node:fs'
import {
  access,
  mkdir,
  readdir,
  readFile,
  realpath,
  stat,
  writeFile,
  chmod,
} from 'node:fs/promises'
import https from 'node:https'
import { homedir, hostname } from 'node:os'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import process from 'node:process'
import tls from 'node:tls'

const MAX_READ_BYTES = 16 * 1024 * 1024
const MAX_GLOB_VISITS = 200_000
// 选工作目录时一层最多列这么多文件夹，与大脑本机的列表同一上限。
const MAX_LISTED_FOLDERS = 1000
const POLL_TIMEOUT_MS = 40_000
const CHUNK_FLUSH_MS = 150
export class NodeOperationError extends Error {
  constructor(code, message) {
    super(message)
    this.code = code
  }
}

function fail(code, message) {
  throw new NodeOperationError(code, message)
}

export function executorHome(env = process.env) {
  return env.PISPER_EXECUTOR_HOME || join(homedir(), '.pisper-executor')
}

// glob → 正则：支持 **、*、?、{a,b} 与 [...]；路径分隔符统一成 /。
export function globToRegExp(pattern) {
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
    } else if (char === '[') {
      const end = text.indexOf(']', index)
      if (end < 0) source += '\\['
      else {
        source += `[${text.slice(index + 1, end).replace(/^!/, '^')}]`
        index = end
      }
    } else source += char.replace(/[.+^$()|{}\\]/g, '\\$&')
  }
  return new RegExp(`^${source}$`)
}

// 命令交给哪个 shell、参数怎么传。Windows 上 cmd.exe 不按 C 运行库的规则解析命令行：
// 照常传参时 Node 会把命令里的 " 转成 \"，cmd 原样收到，带引号的命令就坏了。
// 所以和 Node 的 shell: true 一样，整条命令用一对引号包起来原样传（/s 会去掉这对引号）。
// 先 chcp 65001 把这个控制台的代码页切到 UTF-8，dir、type 这类内建命令的中文输出才不会乱码。
export function shellInvocation(
  command,
  { platform = process.platform, env = process.env, exists = existsSync } = {},
) {
  if (platform === 'win32')
    return {
      file: env.ComSpec || 'cmd.exe',
      args: ['/d', '/s', '/c', `"chcp 65001 >nul & ${command}"`],
      verbatim: true,
    }
  return {
    file: exists('/bin/bash') ? '/bin/bash' : '/bin/sh',
    args: ['-c', command],
    verbatim: false,
  }
}

export class ExecutorNode {
  constructor({ roots, shell = true }) {
    this.roots = roots.map((root) => resolve(root))
    this.shell = shell
    this.running = new Map()
    this.realRoots = null
  }

  report() {
    return {
      roots: this.roots,
      capabilities: { shell: this.shell },
      platform: process.platform,
    }
  }

  async #allowedRoots() {
    if (this.realRoots) return this.realRoots
    const roots = []
    for (const root of this.roots) {
      try {
        roots.push(await realpath(root))
      } catch {}
    }
    this.realRoots = roots
    return roots
  }

  // 路径必须是绝对路径，且按真实路径（最近一个已存在的上级解析符号链接）落在允许目录里。
  async allowed(path) {
    const text = String(path ?? '')
    if (!text || text.includes('\0') || !isAbsolute(text))
      fail('path_invalid', '路径必须是绝对路径。')
    let existing = resolve(text)
    const rest = []
    for (;;) {
      try {
        existing = await realpath(existing)
        break
      } catch {
        const parent = dirname(existing)
        if (parent === existing) fail('path_not_allowed', `不在允许的目录里：${text}`)
        rest.unshift(existing.slice(parent.length).replace(/^[\\/]+/, ''))
        existing = parent
      }
    }
    const real = rest.length ? join(existing, ...rest) : existing
    const roots = await this.#allowedRoots()
    if (!roots.some((root) => real === root || real.startsWith(`${root}${sep}`)))
      fail('path_not_allowed', `不在允许的目录里：${text}`)
    return real
  }

  async #shell(id, { command, cwd, timeoutMs }, emit) {
    if (!this.shell) fail('shell_disabled', '这台电脑没有允许执行命令（--no-shell）。')
    const directory = await this.allowed(cwd)
    const windows = process.platform === 'win32'
    const shell = shellInvocation(command)
    const child = spawn(shell.file, shell.args, {
      cwd: directory,
      detached: !windows,
      windowsHide: true,
      windowsVerbatimArguments: shell.verbatim,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
    // 杀整棵进程树：Windows 上只杀 cmd.exe 会留下它启动的程序，继续占着输出管道和工作目录。
    const kill = () => {
      try {
        if (windows && child.pid)
          spawn('taskkill', ['/pid', String(child.pid), '/T', '/F'], {
            stdio: 'ignore',
            windowsHide: true,
          }).once('error', () => child.kill('SIGKILL'))
        else if (child.pid) process.kill(-child.pid, 'SIGKILL')
        else child.kill('SIGKILL')
      } catch {}
    }
    this.running.set(id, kill)
    const timer = setTimeout(
      kill,
      Math.min(Math.max(1000, Number(timeoutMs) || 600_000), 3_600_000),
    )
    child.stdout.on('data', (data) => emit(data))
    child.stderr.on('data', (data) => emit(data))
    return new Promise((resolveResult, reject) => {
      child.once('error', (error) => {
        clearTimeout(timer)
        this.running.delete(id)
        reject(new NodeOperationError('command_failed', error.message))
      })
      child.once('close', (code, signal) => {
        clearTimeout(timer)
        this.running.delete(id)
        resolveResult({
          exitCode: typeof code === 'number' ? code : signal ? 128 + 9 : null,
        })
      })
    })
  }

  async #glob({ pattern, cwd, ignore, limit }) {
    const base = await this.allowed(cwd)
    const matcher = globToRegExp(pattern)
    const ignored = (Array.isArray(ignore) ? ignore : []).map(globToRegExp)
    const max = Math.min(Math.max(1, Number(limit) || 1000), 10_000)
    const found = []
    let visits = 0
    const walk = async (directory) => {
      if (found.length >= max || visits >= MAX_GLOB_VISITS) return
      let entries
      try {
        entries = await readdir(directory, { withFileTypes: true })
      } catch {
        return
      }
      for (const entry of entries) {
        visits += 1
        if (found.length >= max || visits >= MAX_GLOB_VISITS) return
        const path = join(directory, entry.name)
        const rel = relative(base, path).replaceAll('\\', '/')
        const probe = entry.isDirectory() ? `${rel}/` : rel
        if (ignored.some((rule) => rule.test(rel) || rule.test(probe))) continue
        if (entry.name === '.git') continue
        if (matcher.test(rel)) found.push(rel)
        if (entry.isDirectory() && !entry.isSymbolicLink()) await walk(path)
      }
    }
    await walk(base)
    return found
  }

  // 列出一层子文件夹，供大脑的界面在这台电脑上点选 Bot 的目录：只给文件夹名和路径，不列文件。
  // 隐藏文件夹不列；符号链接指向允许范围外的不列（点进去也会被拒）。到允许的根就不再往上。
  async #folders({ path }) {
    const base = await this.allowed(path)
    const roots = await this.#allowedRoots()
    const folders = []
    for (const entry of await readdir(base, { withFileTypes: true })) {
      if (entry.name.startsWith('.')) continue
      const child = join(base, entry.name)
      if (entry.isDirectory()) folders.push({ name: entry.name, path: child })
      else if (entry.isSymbolicLink()) {
        const target = await this.allowed(child).catch(() => null)
        if (target && (await stat(target).catch(() => null))?.isDirectory())
          folders.push({ name: entry.name, path: child })
      }
    }
    folders.sort((left, right) =>
      left.name.localeCompare(right.name, undefined, { numeric: true, sensitivity: 'base' }),
    )
    const parent = dirname(base)
    const parentAllowed =
      !roots.includes(base) &&
      roots.some((root) => parent === root || parent.startsWith(`${root}${sep}`))
    return {
      path: base,
      parent: parentAllowed ? parent : '',
      folders: folders.slice(0, MAX_LISTED_FOLDERS),
      truncated: folders.length > MAX_LISTED_FOLDERS,
    }
  }

  // 执行一个请求；emit 用来流式交回命令输出。返回 { ok, value } 或 { ok: false, error }。
  async handle(request, emit = () => {}) {
    const { id, op } = request
    const args = request.args && typeof request.args === 'object' ? request.args : {}
    try {
      switch (op) {
        case 'cancel':
          this.running.get(String(args.id))?.()
          return { ok: true, value: null }
        case 'shell':
          return { ok: true, value: await this.#shell(id, args, emit) }
        case 'fs.read': {
          const path = await this.allowed(args.path)
          const info = await stat(path)
          if (info.size > MAX_READ_BYTES) fail('file_too_large', '文件太大，超过 16 MB。')
          return { ok: true, value: (await readFile(path)).toString('base64') }
        }
        case 'fs.access': {
          const path = await this.allowed(args.path)
          await access(path, args.mode === 'rw' ? 6 : 4)
          return { ok: true, value: null }
        }
        case 'fs.write': {
          const path = await this.allowed(args.path)
          await mkdir(dirname(path), { recursive: true })
          await writeFile(path, String(args.content ?? ''), 'utf8')
          return { ok: true, value: null }
        }
        case 'fs.mkdir':
          await mkdir(await this.allowed(args.path), { recursive: true })
          return { ok: true, value: null }
        case 'fs.stat': {
          const path = await this.allowed(args.path)
          const info = await stat(path).catch(() => null)
          return {
            ok: true,
            value: { exists: Boolean(info), isDirectory: Boolean(info?.isDirectory()) },
          }
        }
        case 'fs.readdir':
          return { ok: true, value: await readdir(await this.allowed(args.path)) }
        case 'fs.glob':
          return { ok: true, value: await this.#glob(args) }
        case 'fs.folders':
          return { ok: true, value: await this.#folders(args) }
        default:
          fail('op_unsupported', `不支持的操作：${op}`)
      }
    } catch (error) {
      return {
        ok: false,
        error: {
          code:
            error instanceof NodeOperationError
              ? error.code
              : error?.code === 'ENOENT'
                ? 'not_found'
                : 'op_failed',
          message: String(error?.message || error).slice(0, 500),
        },
      }
    }
  }

  cancelAll() {
    for (const kill of this.running.values()) kill()
  }
}

// HTTPS 客户端：先取一次大脑的证书，确认指纹与配对时一致，之后把这张证书当作唯一可信的 CA。
export function normalizeFingerprint(value) {
  const hex = String(value || '')
    .replace(/^SHA256:/i, '')
    .replace(/[^0-9a-f]/gi, '')
    .toUpperCase()
  return hex.length === 64 ? `SHA256:${hex.match(/../g).join(':')}` : ''
}

async function pinnedCertificate(brain, fingerprint) {
  const url = new URL(brain)
  return new Promise((resolveCert, reject) => {
    const socket = tls.connect(
      {
        host: url.hostname.replace(/^\[|\]$/g, ''),
        port: Number(url.port || 443),
        servername: /^[\d.]+$|:/.test(url.hostname) ? undefined : url.hostname,
        rejectUnauthorized: false,
      },
      () => {
        const raw = socket.getPeerCertificate(true)?.raw
        socket.end()
        if (!raw) return reject(new Error('读不到大脑的证书。'))
        const cert = new X509Certificate(raw)
        if (normalizeFingerprint(cert.fingerprint256) !== normalizeFingerprint(fingerprint))
          return reject(new Error('大脑的证书指纹和配对时不一致，已停止连接。'))
        resolveCert(cert.toString())
      },
    )
    socket.setTimeout(15_000, () => socket.destroy(new Error('连接大脑超时。')))
    socket.once('error', reject)
  })
}

export async function createBrainClient({ brain, fingerprint }) {
  const url = new URL(brain)
  if (url.protocol !== 'https:') throw new Error('大脑地址必须是 https://（远程访问地址）。')
  const expected = normalizeFingerprint(fingerprint)
  if (!expected) throw new Error('证书指纹格式不对，应形如 SHA256:AB:CD:…')
  const ca = await pinnedCertificate(brain, expected)
  const agent = new https.Agent({
    ca,
    keepAlive: true,
    // 大脑的自签证书不带这台机器认识的主机名；只认指纹，不认主机名。
    checkServerIdentity: (_host, cert) =>
      normalizeFingerprint(cert.fingerprint256) === expected
        ? undefined
        : new Error('大脑的证书指纹和配对时不一致。'),
  })
  return {
    post(path, body, { token = '', timeoutMs = 30_000 } = {}) {
      const payload = Buffer.from(JSON.stringify(body ?? {}))
      return new Promise((resolveResponse, reject) => {
        const request = https.request(
          new URL(path, url),
          {
            method: 'POST',
            agent,
            headers: {
              'Content-Type': 'application/json',
              'Content-Length': payload.length,
              ...(token ? { Authorization: `Bearer ${token}` } : {}),
            },
          },
          (response) => {
            const chunks = []
            response.on('data', (chunk) => chunks.push(chunk))
            response.on('end', () => {
              let data = {}
              try {
                data = JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}')
              } catch {}
              if ((response.statusCode || 500) >= 400) {
                const error = new Error(data.error || `大脑返回 ${response.statusCode}`)
                error.status = response.statusCode
                error.code = data.code
                reject(error)
              } else resolveResponse(data)
            })
          },
        )
        request.setTimeout(timeoutMs, () => request.destroy(new Error('请求大脑超时。')))
        request.once('error', reject)
        request.end(payload)
      })
    },
    close() {
      agent.destroy()
    },
  }
}

// 连上大脑干活：一直挂着一次轮询，领到请求就并行执行，结果与输出尽快交回。
export async function runExecutorLoop({ client, token, node, signal, log = () => {} }) {
  const outbox = []
  let flushing = null
  const flush = async () => {
    if (flushing || !outbox.length) return flushing
    const results = outbox.splice(0, outbox.length)
    flushing = client
      .post('/api/executor/results', { results }, { token })
      .catch((error) => log(`交回结果失败：${error.message}`))
      .finally(() => {
        flushing = null
        if (outbox.length) void flush()
      })
    return flushing
  }
  let flushTimer = null
  const queueChunk = (id, data) => {
    outbox.push({ id, chunk: Buffer.from(data).toString('base64') })
    flushTimer ??= setTimeout(() => {
      flushTimer = null
      void flush()
    }, CHUNK_FLUSH_MS)
  }
  let delay = 1000
  while (!signal?.aborted) {
    try {
      const { requests = [] } = await client.post(
        '/api/executor/poll',
        { ...node.report() },
        { token, timeoutMs: POLL_TIMEOUT_MS },
      )
      delay = 1000
      for (const request of Array.isArray(requests) ? requests : []) {
        void node
          .handle(request, (data) => queueChunk(request.id, data))
          .then((result) => {
            if (request.op === 'cancel') return
            outbox.push({ id: request.id, ...result })
            void flush()
          })
      }
    } catch (error) {
      if (error.status === 401) throw new Error('大脑不认这台电脑了（可能已被移除），请重新配对。')
      log(`连接大脑失败：${error.message}，${Math.round(delay / 1000)} 秒后重试。`)
      await new Promise((done) => setTimeout(done, delay))
      delay = Math.min(delay * 2, 30_000)
    }
  }
  node.cancelAll()
}

function parseArgs(argv) {
  const options = { roots: [], shell: true }
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index]
    const next = () => argv[++index] ?? ''
    if (arg === '--brain') options.brain = next()
    else if (arg === '--fingerprint') options.fingerprint = next()
    else if (arg === '--code') options.code = next()
    else if (arg === '--root') options.roots.push(resolve(next()))
    else if (arg === '--name') options.name = next()
    else if (arg === '--no-shell') options.shell = false
    else if (arg === '--help' || arg === '-h') options.help = true
    else if (!options.command) options.command = arg
    else throw new Error(`不认识的参数：${arg}`)
  }
  return options
}

const HELP = `pisper executor —— 让另一台电脑上的 Pisper Bot 在这台电脑上干活

  pisper executor pair --brain <https 地址> --fingerprint <SHA256:…> --code <配对码> --root <目录> [--root …] [--name 名字] [--no-shell]
      用 Pisper「Bot › 电脑 › 添加电脑」里显示的配对码配对。只有 --root 列出的目录可以读写。
  pisper executor
      连上大脑开始干活，Ctrl+C 停止。

配对信息保存在 ${executorHome()}（可用 PISPER_EXECUTOR_HOME 修改）。`

async function readConfig(home) {
  try {
    return JSON.parse(await readFile(join(home, 'config.json'), 'utf8'))
  } catch {
    return null
  }
}

export async function executorCommand(argv, { env = process.env, log = console.log } = {}) {
  const options = parseArgs(argv)
  if (options.help) {
    log(HELP)
    return 0
  }
  const home = executorHome(env)
  if (options.command === 'pair') {
    if (!options.brain || !options.fingerprint || !options.code || !options.roots.length)
      throw new Error('配对需要 --brain、--fingerprint、--code 和至少一个 --root。')
    for (const root of options.roots)
      if (!(await stat(root).catch(() => null))?.isDirectory())
        throw new Error(`目录不存在：${root}`)
    const node = new ExecutorNode({
      roots: options.roots,
      shell: options.shell,
    })
    const client = await createBrainClient(options)
    try {
      const paired = await client.post('/api/executor/pair', {
        code: options.code,
        name: options.name || hostname(),
        ...node.report(),
      })
      await mkdir(home, { recursive: true, mode: 0o700 })
      const config = {
        brain: options.brain,
        fingerprint: normalizeFingerprint(options.fingerprint),
        token: paired.token,
        roots: options.roots,
        shell: options.shell,
      }
      await writeFile(join(home, 'config.json'), `${JSON.stringify(config, null, 2)}\n`, {
        mode: 0o600,
      })
      await chmod(join(home, 'config.json'), 0o600).catch(() => {})
      log(`已配对为「${paired.environment?.name || ''}」。运行 pisper executor 开始干活。`)
    } finally {
      client.close()
    }
    return 0
  }
  if (options.command && options.command !== 'run')
    throw new Error(`不认识的命令：${options.command}`)
  const config = await readConfig(home)
  if (!config?.token)
    throw new Error('还没有配对。先运行 pisper executor pair …（--help 查看用法）')
  const node = new ExecutorNode({
    roots: config.roots,
    shell: config.shell !== false,
  })
  const client = await createBrainClient(config)
  const controller = new AbortController()
  for (const name of ['SIGINT', 'SIGTERM']) process.once(name, () => controller.abort())
  log(`已连上 ${config.brain}，允许的目录：${config.roots.join('、')}。Ctrl+C 停止。`)
  try {
    await runExecutorLoop({ client, token: config.token, node, signal: controller.signal, log })
  } finally {
    client.close()
  }
  return 0
}
