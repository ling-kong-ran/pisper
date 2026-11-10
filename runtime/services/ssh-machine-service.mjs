// SSH 机器的登记表：存怎么连（地址、端口、默认目录、登录方式）和连上后探测到的系统。
// 密钥登录不存任何凭据，全部留给系统的 ssh；用户选了密码登录时，密码单独存进 pisper-ssh-secrets.json
// （和 Provider 的多 Key 一样原子写入、0600），机器列表、接口响应、会话文件里都只有“设了密码”这一个布尔值。
import { randomBytes } from 'node:crypto'
import { join } from 'node:path'
import { pisperError } from '../../shared/errors.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { ExecutorError } from './bots/executor.mjs'
import { normalizeSystem, SshExecutor } from './ssh-executor.mjs'

const STORE_VERSION = 1
const SECRETS_VERSION = 1
const MAX_MACHINES = 32
const MAX_NAME_CHARS = 40
const MAX_TARGET_CHARS = 255
const MAX_PATH_CHARS = 1024
const MAX_PASSWORD_CHARS = 1024
// 和执行器里的 socket 路径上限一致：数据目录太长时（macOS 的 Application Support）改用 /tmp 下自己的目录。
const MAX_MUX_DIRECTORY_CHARS = 45
const ID_PATTERN = /^ssh_[A-Za-z0-9_-]{8,32}$/
// user@host 或 ~/.ssh/config 里的别名。白名单而不是黑名单：开头的 - 会被 ssh 当成选项（-oProxyCommand=…），
// 主机名里的 shell 元字符配合 ProxyCommand 的 %h 也能执行命令。IPv6 地址放在方括号里。
const TARGET_PATTERN =
  /^(?:[A-Za-z0-9_][A-Za-z0-9._-]*@)?(?:[A-Za-z0-9_][A-Za-z0-9._-]*|\[[0-9A-Fa-f:.]+\])$/

export function isSshMachineId(value) {
  return ID_PATTERN.test(String(value))
}

function cleanText(value, max) {
  return String(value ?? '')
    .replace(/[\u0000-\u001f\u007f]/g, ' ')
    .trim()
    .slice(0, max)
}

export function normalizeSshConnection(input) {
  const value = input && typeof input === 'object' ? input : {}
  const target = String(value.target ?? '').trim()
  if (!target || target.length > MAX_TARGET_CHARS || !TARGET_PATTERN.test(target))
    throw new ExecutorError(
      'invalid_target',
      'Address must be user@host, host or an alias from ~/.ssh/config.',
    )
  let port = null
  if (value.port !== undefined && value.port !== null && value.port !== '') {
    port = Number(value.port)
    if (!Number.isInteger(port) || port < 1 || port > 65_535)
      throw new ExecutorError('invalid_port', 'Port must be an integer from 1 to 65535.')
  }
  return { target, port }
}

// 登录方式：只有明确选了 password 才是密码登录；旧记录没有这个字段，按密钥处理。
export function normalizeSshAuth(value) {
  return value === 'password' ? 'password' : 'key'
}

// 密码按不可信输入校验：askpass 按行交给 ssh，换行和 NUL 会截断或改变它，直接拒绝。错误里不带密码本身。
export function normalizeSshPassword(value) {
  if (value === undefined || value === null || value === '') return ''
  if (typeof value !== 'string' || value.length > MAX_PASSWORD_CHARS || /[\0\r\n]/.test(value))
    throw pisperError(
      'invalid',
      'login',
      `password must be at most ${MAX_PASSWORD_CHARS} characters, without line breaks`,
    )
  return value
}

// /tmp 是共享目录：只用带自己 uid 的子目录，执行器每次用之前还会确认它属于自己、权限是 0700。
export function muxDirectoryFor(
  dataDir,
  { platform = process.platform, uid = process.getuid?.() } = {},
) {
  if (platform === 'win32') return ''
  const preferred = join(dataDir, 'ssh-mux')
  if (preferred.length <= MAX_MUX_DIRECTORY_CHARS) return preferred
  return Number.isInteger(uid) ? `/tmp/pisper-ssh-${uid}` : ''
}

function normalizeStored(value) {
  if (!value || typeof value !== 'object' || !isSshMachineId(value.id)) return null
  try {
    const connection = normalizeSshConnection(value)
    const lastCheck =
      value.lastCheck && typeof value.lastCheck === 'object'
        ? {
            ok: value.lastCheck.ok === true,
            at: String(value.lastCheck.at || ''),
            code: cleanText(value.lastCheck.code, 40),
            platform: cleanText(value.lastCheck.platform, 40),
          }
        : null
    const system = normalizeSystem(value.system, lastCheck)
    return {
      id: value.id,
      name: cleanText(value.name, MAX_NAME_CHARS) || connection.target,
      ...connection,
      auth: normalizeSshAuth(value.auth),
      defaultCwd: cleanText(value.defaultCwd, MAX_PATH_CHARS),
      addedAt: String(value.addedAt || ''),
      lastCheck,
      ...(system ? { system } : {}),
    }
  } catch {
    return null
  }
}

// 机器的密码：只在这个文件里，只在要连的那一刻交给执行器。
class SshSecretStore {
  constructor(path) {
    this.path = path
    this.passwords = new Map()
    this.write = Promise.resolve()
  }

  async init() {
    const stored = await readJson(this.path, { version: SECRETS_VERSION, passwords: {} })
    this.passwords.clear()
    const passwords =
      stored?.passwords && typeof stored.passwords === 'object' ? stored.passwords : {}
    for (const [id, password] of Object.entries(passwords)) {
      if (!isSshMachineId(id)) continue
      try {
        const value = normalizeSshPassword(password)
        if (value) this.passwords.set(id, value)
      } catch {}
    }
  }

  has(id) {
    return this.passwords.has(id)
  }

  get(id) {
    return this.passwords.get(id) || ''
  }

  persist() {
    const snapshot = { version: SECRETS_VERSION, passwords: Object.fromEntries(this.passwords) }
    this.write = this.write
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot, { mode: 0o600 }))
    return this.write
  }

  async set(id, password) {
    if (this.passwords.get(id) === password) return
    this.passwords.set(id, password)
    await this.persist()
  }

  async delete(id) {
    if (!this.passwords.delete(id)) return
    await this.persist()
  }
}

export class SshMachineService {
  constructor({
    path,
    dataDir,
    secretsPath = join(dataDir, 'pisper-ssh-secrets.json'),
    extraArguments = [],
    now = () => new Date().toISOString(),
    muxDirectory = muxDirectoryFor(dataDir),
  }) {
    this.path = path
    this.muxDirectory = muxDirectory
    this.extraArguments = extraArguments
    this.now = now
    this.machines = new Map()
    this.secrets = new SshSecretStore(secretsPath)
    this.write = Promise.resolve()
  }

  async init() {
    const stored = await readJson(this.path, { version: STORE_VERSION, machines: [] })
    this.machines.clear()
    for (const raw of Array.isArray(stored?.machines) ? stored.machines : []) {
      const machine = normalizeStored(raw)
      if (machine) this.machines.set(machine.id, machine)
    }
    await this.secrets.init()
    // 机器已经不在的密码不留着。
    for (const id of [...this.secrets.passwords.keys()])
      if (!this.machines.has(id)) await this.secrets.delete(id)
  }

  persist() {
    const snapshot = { version: STORE_VERSION, machines: [...this.machines.values()] }
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  // 给界面和接口的形状：密码只以 passwordSet 出现。
  publicMachine(machine) {
    return {
      id: machine.id,
      name: machine.name,
      target: machine.target,
      port: machine.port,
      auth: machine.auth,
      passwordSet: machine.auth === 'password' && this.secrets.has(machine.id),
      defaultCwd: machine.defaultCwd,
      addedAt: machine.addedAt,
      lastCheck: machine.lastCheck ? { ...machine.lastCheck } : null,
      system: machine.system ? { ...machine.system } : null,
    }
  }

  list() {
    return [...this.machines.values()].map((machine) => this.publicMachine(machine))
  }

  get(id) {
    return this.machines.get(id) || null
  }

  // 机器被移除后还有会话指着它：执行器照常给，第一个操作会以“已移除”失败。
  executor(id) {
    const machine = this.machines.get(id)
    if (!machine) return new RemovedMachineExecutor(id)
    return this.executorFor(machine, {
      isAvailable: () => this.machines.has(id),
      password: () => this.secrets.get(id),
      onSystem: (system) => this.recordSystem(id, system),
    })
  }

  executorFor(machine, { isAvailable, password, onSystem } = {}) {
    return new SshExecutor({
      machine,
      muxDirectory: this.muxDirectory,
      extraArguments: this.extraArguments,
      ...(isAvailable ? { isAvailable } : {}),
      ...(password ? { password } : {}),
      ...(onSystem ? { onSystem } : {}),
    })
  }

  // 还没保存的连接也能测：添加表单在保存前就给出结果。编辑已有机器时没重新输入密码，就用存着的那个。
  async test(input) {
    const connection = normalizeSshConnection(input)
    const auth = normalizeSshAuth(input?.auth)
    const typed = auth === 'password' ? normalizeSshPassword(input?.password) : ''
    const existing = isSshMachineId(input?.id) ? String(input.id) : ''
    const password = typed || (auth === 'password' && existing ? this.secrets.get(existing) : '')
    if (auth === 'password' && !password)
      throw pisperError('invalid', 'login', 'enter the password')
    const executor = this.executorFor(
      { id: 'ssh_unsaved0', name: connection.target, ...connection, auth },
      { password: () => password },
    )
    return executor.probe()
  }

  async add(input) {
    const connection = normalizeSshConnection(input)
    const auth = normalizeSshAuth(input?.auth)
    const password = auth === 'password' ? normalizeSshPassword(input?.password) : ''
    if (auth === 'password' && !password)
      throw pisperError('invalid', 'login', 'enter the password')
    if (this.machines.size >= MAX_MACHINES)
      throw new ExecutorError('too_many_machines', `Machine limit reached: max ${MAX_MACHINES}.`)
    const machine = {
      id: `ssh_${randomBytes(9).toString('base64url')}`,
      name: cleanText(input?.name, MAX_NAME_CHARS) || connection.target,
      ...connection,
      auth,
      defaultCwd: cleanText(input?.defaultCwd, MAX_PATH_CHARS),
      addedAt: this.now(),
      lastCheck: null,
    }
    // 先存密码再登记机器：中途失败时最多留下一条启动时会清掉的孤立密码，不会有缺密码的密码机器。
    if (password) await this.secrets.set(machine.id, password)
    this.machines.set(machine.id, machine)
    await this.persist()
    return this.publicMachine(machine)
  }

  // 编辑：密码登录时不填密码 = 保留原来的；填了 = 换掉；改回密钥登录 = 删掉存着的密码。
  // 地址、端口或登录方式变了，之前的连通结果和探测到的系统都不再算数。
  async update(id, input) {
    const machine = this.machines.get(id)
    if (!machine) throw new ExecutorError('machine_not_found', 'Machine was removed.')
    const connection = normalizeSshConnection({
      target: input?.target ?? machine.target,
      port: input?.port === undefined ? machine.port : input.port,
    })
    const auth = input?.auth === undefined ? machine.auth : normalizeSshAuth(input.auth)
    const password = auth === 'password' ? normalizeSshPassword(input?.password) : ''
    if (auth === 'password' && !password && !this.secrets.has(id))
      throw pisperError('invalid', 'login', 'enter the password')
    const changed =
      connection.target !== machine.target ||
      connection.port !== machine.port ||
      auth !== machine.auth ||
      (Boolean(password) && password !== this.secrets.get(id))
    if (auth === 'password' && password) await this.secrets.set(id, password)
    if (auth === 'key') await this.secrets.delete(id)
    Object.assign(machine, connection, { auth })
    if (input?.name !== undefined)
      machine.name = cleanText(input.name, MAX_NAME_CHARS) || connection.target
    if (input?.defaultCwd !== undefined)
      machine.defaultCwd = cleanText(input.defaultCwd, MAX_PATH_CHARS)
    if (changed) {
      machine.lastCheck = null
      delete machine.system
    }
    await this.persist()
    return this.publicMachine(machine)
  }

  async remove(id) {
    if (!this.machines.delete(id)) return false
    await this.persist()
    await this.secrets.delete(id)
    return true
  }

  async recordCheck(id, check) {
    const machine = this.machines.get(id)
    if (!machine) return
    machine.lastCheck = {
      ok: check.ok === true,
      at: this.now(),
      code: check.code || '',
      platform: check.platform || '',
    }
    await this.persist()
  }

  // 探测到的系统记在机器上：之后的会话直接按它构造命令，不再每次探测。
  async recordSystem(id, system) {
    const machine = this.machines.get(id)
    const normalized = normalizeSystem(system)
    if (!machine || !normalized) return
    if (JSON.stringify(machine.system) === JSON.stringify(normalized)) return
    machine.system = normalized
    await this.persist()
  }

  // 实际连上过（选目录、建会话）：列表里的状态跟着更新，不要求用户再点一次测试。
  async recordReachable(id) {
    const machine = this.machines.get(id)
    if (machine?.lastCheck?.ok) return
    await this.recordCheck(id, {
      ok: true,
      platform: machine?.lastCheck?.platform || machine?.system?.name || '',
    })
  }

  // 真实连一次，结果记下来：列表据此显示能不能连。失败不是异常，分类后交给界面。
  async check(id) {
    const executor = this.executor(id)
    try {
      const result = await executor.probe()
      await this.recordCheck(id, { ok: true, platform: result.platform })
      return { ok: true, ...result }
    } catch (error) {
      const code = error instanceof ExecutorError ? error.code : 'connection_failed'
      await this.recordCheck(id, { ok: false, code })
      return { ok: false, code, message: error instanceof Error ? error.message : String(error) }
    }
  }

  async dispose() {
    await this.write.catch(() => {})
    await this.secrets.write.catch(() => {})
  }
}

class RemovedMachineExecutor {
  constructor(id) {
    this.id = id
    this.kind = 'ssh'
    this.name = ''
    this.capabilities = Object.freeze({ shell: true, browser: false })
  }

  online() {
    return false
  }

  operations() {
    const fail = async () => {
      throw new ExecutorError('machine_not_found', 'Machine was removed.')
    }
    return {
      read: { readFile: fail, access: fail },
      edit: { readFile: fail, writeFile: fail, access: fail },
      write: { writeFile: fail, mkdir: fail },
      ls: { exists: fail, stat: fail, readdir: fail },
      find: { exists: fail, glob: fail },
      bash: { exec: fail },
    }
  }

  async resolveDirectory() {
    throw new ExecutorError('machine_not_found', 'Machine was removed.')
  }

  async probe() {
    throw new ExecutorError('machine_not_found', 'Machine was removed.')
  }
}
