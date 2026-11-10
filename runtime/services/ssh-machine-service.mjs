// SSH 机器的登记表：存怎么连（地址、端口、默认目录、登录方式）和连上后探测到的系统。
// 密钥登录不存任何凭据，全部留给系统的 ssh；用户选了密码登录时，密码单独存进 pisper-ssh-secrets.json
// （和 Provider 的多 Key 一样原子写入、0600），机器列表、接口响应、会话文件里都只有“设了密码”这一个布尔值。
// 密码不常驻内存：只在要连的那一刻从文件读出，交给这一次连接的密码通道，用完就放掉。
// 密码机器还存着用户核对过指纹的主机公钥（hostKey）：连接只认它，没确认过就不连（见 ssh-host-key.mjs）。
import { randomBytes } from 'node:crypto'
import { readdir, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { pisperError } from '../../shared/errors.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { ExecutorError } from './bots/executor.mjs'
import { normalizeSystem, SshExecutor } from './ssh-executor.mjs'
import { normalizeHostKey, sameHostKey } from './ssh-host-key.mjs'

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
// 取到的主机公钥等用户核对的时间：过了就要重新取。
const FETCHED_HOST_KEY_TTL_MS = 10 * 60_000
const MAX_FETCHED_HOST_KEYS = 32
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
    const auth = normalizeSshAuth(value.auth)
    // 只有密码机器留着主机公钥；读回时重新校验，指纹每次从公钥算。
    const hostKey = auth === 'password' ? normalizeHostKey(value.hostKey) : null
    return {
      id: value.id,
      name: cleanText(value.name, MAX_NAME_CHARS) || connection.target,
      ...connection,
      auth,
      ...(hostKey ? { hostKey: { type: hostKey.type, key: hostKey.key } } : {}),
      defaultCwd: cleanText(value.defaultCwd, MAX_PATH_CHARS),
      addedAt: String(value.addedAt || ''),
      lastCheck,
      ...(system ? { system } : {}),
    }
  } catch {
    return null
  }
}

// 机器的密码：只在这个文件里。内存里只记“哪些机器存了密码”，密码本身每次用到时从文件读，
// 不在这个常驻对象里留一份。读、改都排在同一条队列上，不会互相覆盖。
class SshSecretStore {
  constructor(path) {
    this.path = path
    this.ids = new Set()
    this.write = Promise.resolve()
  }

  async #read() {
    const stored = await readJson(this.path, { version: SECRETS_VERSION, passwords: {} })
    const passwords = new Map()
    const raw = stored?.passwords && typeof stored.passwords === 'object' ? stored.passwords : {}
    for (const [id, password] of Object.entries(raw)) {
      if (!isSshMachineId(id)) continue
      try {
        const value = normalizeSshPassword(password)
        if (value) passwords.set(id, value)
      } catch {}
    }
    return passwords
  }

  #queue(task) {
    const next = this.write.catch(() => {}).then(task)
    this.write = next
    return next
  }

  async init() {
    this.ids = new Set((await this.#read()).keys())
  }

  has(id) {
    return this.ids.has(id)
  }

  // 只在要连的那一刻调用；返回值由调用方用完即弃。
  get(id) {
    if (!this.ids.has(id)) return Promise.resolve('')
    return this.#queue(async () => (await this.#read()).get(id) || '')
  }

  #update(change) {
    return this.#queue(async () => {
      const passwords = await this.#read()
      if (!change(passwords)) return
      await writeJsonAtomic(
        this.path,
        { version: SECRETS_VERSION, passwords: Object.fromEntries(passwords) },
        { mode: 0o600 },
      )
      this.ids = new Set(passwords.keys())
    })
  }

  set(id, password) {
    return this.#update((passwords) => {
      if (passwords.get(id) === password) return false
      passwords.set(id, password)
      return true
    })
  }

  delete(id) {
    if (!this.ids.has(id)) return Promise.resolve()
    return this.#update((passwords) => passwords.delete(id))
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
    scratchDirectory = join(dataDir, 'ssh-password'),
  }) {
    this.path = path
    this.muxDirectory = muxDirectory
    // 密码登录每次连接的临时文件（空配置、known_hosts、密码通道）建在这里，用完即删。
    this.scratchDirectory = scratchDirectory
    this.extraArguments = extraArguments
    this.now = now
    this.machines = new Map()
    this.secrets = new SshSecretStore(secretsPath)
    this.write = Promise.resolve()
    // 存着的密码被对方拒绝过的机器：之后的连接直接失败，不再拿同一个密码反复去试（每个工具调用都新开一次
    // 连接，几次就能锁住账户或被 fail2ban 封掉）。改了密码或地址、或用户点了测试就解除。只记在内存里：
    // 重启后会再试一次，再被拒绝就又记上。
    this.authRefused = new Set()
    // 这边刚从某个地址取到的主机公钥（地址 → 公钥与时间）：保存时只认这里有的那一把。
    this.fetchedHostKeys = new Map()
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
    for (const id of [...this.secrets.ids])
      if (!this.machines.get(id) || this.machines.get(id).auth !== 'password')
        await this.secrets.delete(id)
    await this.#clearScratch()
  }

  // 上次没来得及删的每次连接的临时目录（Runtime 崩溃、被杀掉时留下的；里面没有密码）：启动时清掉。
  async #clearScratch() {
    const entries = await readdir(this.scratchDirectory, { withFileTypes: true }).catch(() => [])
    for (const entry of entries)
      if (entry.isDirectory() && entry.name.startsWith('pw-'))
        await rm(join(this.scratchDirectory, entry.name), { recursive: true, force: true }).catch(
          () => {},
        )
  }

  persist() {
    const snapshot = { version: STORE_VERSION, machines: [...this.machines.values()] }
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  // 给界面和接口的形状：密码只以 passwordSet 出现；主机公钥只给类型和指纹（公钥本体留在这边）。
  publicMachine(machine) {
    const hostKey = normalizeHostKey(machine.hostKey)
    return {
      id: machine.id,
      name: machine.name,
      target: machine.target,
      port: machine.port,
      auth: machine.auth,
      passwordSet: machine.auth === 'password' && this.secrets.has(machine.id),
      hostKey: hostKey ? { type: hostKey.type, fingerprint: hostKey.fingerprint } : null,
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
      authBlocked: () => this.authRefused.has(id),
      onAuthFailed: () => this.authRefused.add(id),
    })
  }

  executorFor(machine, { isAvailable, password, onSystem, authBlocked, onAuthFailed } = {}) {
    return new SshExecutor({
      machine,
      muxDirectory: this.muxDirectory,
      scratchDirectory: this.scratchDirectory,
      extraArguments: this.extraArguments,
      ...(isAvailable ? { isAvailable } : {}),
      ...(password ? { password } : {}),
      ...(onSystem ? { onSystem } : {}),
      ...(authBlocked ? { authBlocked } : {}),
      ...(onAuthFailed ? { onAuthFailed } : {}),
    })
  }

  // 存着的密码和确认过的主机公钥都只属于存它们时的那个地址和端口：地址或端口不一样时不能拿来用，
  // 否则一次“测试连接”或一次改地址就能把密码送到另一台主机上，或者拿旧主机的指纹去认新主机。
  #sameAddress(id, connection) {
    const machine = this.machines.get(id)
    return Boolean(
      machine &&
      machine.auth === 'password' &&
      machine.target === connection.target &&
      machine.port === connection.port,
    )
  }

  #savedHostKeyFor(id, connection) {
    return this.#sameAddress(id, connection)
      ? normalizeHostKey(this.machines.get(id).hostKey)
      : null
  }

  #addressKey(connection) {
    return `${connection.target.toLowerCase()}|${connection.port ?? ''}`
  }

  // 记下这边刚从这个地址取到的公钥，等用户核对。
  rememberFetchedHostKey(connection, hostKey) {
    const key = this.#addressKey(connection)
    this.fetchedHostKeys.delete(key)
    this.fetchedHostKeys.set(key, { type: hostKey.type, key: hostKey.key, at: Date.now() })
    while (this.fetchedHostKeys.size > MAX_FETCHED_HOST_KEYS)
      this.fetchedHostKeys.delete(this.fetchedHostKeys.keys().next().value)
  }

  // 这次用哪把主机公钥：请求里带了就是用户刚确认的那把，没带就用存着的（地址没变时）。
  // 请求里带的公钥不能是随便一把：要么就是这台机器已经存着的，要么是这边刚从这个地址取到、等用户核对的那一把。
  // 不然调接口的一方可以直接塞一把自己的公钥，让之后的连接去认冒充的主机。
  #confirmedHostKey(input, id, connection) {
    const saved = id ? this.#savedHostKeyFor(id, connection) : null
    if (input?.hostKey !== undefined && input.hostKey !== null) {
      const given = normalizeHostKey(input.hostKey)
      if (!given) throw pisperError('invalid', 'login', 'the host key is not a valid public key')
      if (saved && sameHostKey(given, saved)) return given
      const fetched = this.fetchedHostKeys.get(this.#addressKey(connection))
      if (
        !fetched ||
        Date.now() - fetched.at > FETCHED_HOST_KEY_TTL_MS ||
        !sameHostKey(given, fetched)
      )
        throw pisperError(
          'invalid',
          'login',
          'get the host fingerprint of this machine again and confirm it',
        )
      return given
    }
    if (!saved)
      throw pisperError(
        'invalid',
        'login',
        'confirm the host fingerprint of this machine before signing in with a password',
      )
    return saved
  }

  // 取一台机器的主机公钥给用户核对指纹：只握手，不带密码也不带密钥，不读这台电脑的 ssh 配置。
  async hostKey(input) {
    const connection = normalizeSshConnection(input)
    const hostKey = await this.executorFor({
      id: 'ssh_unsaved0',
      name: connection.target,
      ...connection,
      auth: 'key',
    }).hostKey()
    this.rememberFetchedHostKey(connection, hostKey)
    return hostKey
  }

  // 存着的密码能不能接着用：地址、端口没变，而且要认的还是存密码时确认的那把主机公钥。
  // 公钥换了（“身份变了”后重新确认）正是最可能有人冒充的时候：和改地址一样，要用户重新输入密码。
  #reusesPassword(id, connection, hostKey) {
    return (
      this.#sameAddress(id, connection) &&
      this.secrets.has(id) &&
      sameHostKey(hostKey, this.machines.get(id).hostKey)
    )
  }

  #passwordAgain(id) {
    const machine = this.machines.get(id)
    return pisperError(
      'invalid',
      'login',
      machine && machine.auth === 'password' && this.secrets.has(id)
        ? 'enter the password again: the saved one belongs to the previous address and host fingerprint'
        : 'enter the password',
    )
  }

  // 还没保存的连接也能测：添加表单在保存前就给出结果。编辑已有机器时没重新输入密码，就用存着的那个——
  // 但只在地址和端口都没变的时候。密码登录要带上用户确认过的主机公钥。
  async test(input) {
    const connection = normalizeSshConnection(input)
    const auth = normalizeSshAuth(input?.auth)
    const existing = isSshMachineId(input?.id) ? String(input.id) : ''
    if (auth !== 'password')
      return this.executorFor({
        id: 'ssh_unsaved0',
        name: connection.target,
        ...connection,
        auth,
      }).probe()
    const typed = normalizeSshPassword(input?.password)
    // 地址变了又没给密码：先说这个（存着的密码不会发到新地址），再谈指纹。
    if (
      !typed &&
      !(existing && this.#sameAddress(existing, connection) && this.secrets.has(existing))
    )
      throw this.#passwordAgain(existing)
    const hostKey = this.#confirmedHostKey(input, existing, connection)
    const saved = existing && this.#reusesPassword(existing, connection, hostKey)
    if (!typed && !saved) throw this.#passwordAgain(existing)
    const executor = this.executorFor(
      {
        id: 'ssh_unsaved0',
        name: connection.target,
        ...connection,
        auth,
        hostKey: { type: hostKey.type, key: hostKey.key },
      },
      { password: () => typed || this.secrets.get(existing) },
    )
    return executor.probe()
  }

  async add(input) {
    const connection = normalizeSshConnection(input)
    const auth = normalizeSshAuth(input?.auth)
    const password = auth === 'password' ? normalizeSshPassword(input?.password) : ''
    if (auth === 'password' && !password)
      throw pisperError('invalid', 'login', 'enter the password')
    const hostKey = auth === 'password' ? this.#confirmedHostKey(input, '', connection) : null
    if (this.machines.size >= MAX_MACHINES)
      throw new ExecutorError('too_many_machines', `Machine limit reached: max ${MAX_MACHINES}.`)
    const machine = {
      id: `ssh_${randomBytes(9).toString('base64url')}`,
      name: cleanText(input?.name, MAX_NAME_CHARS) || connection.target,
      ...connection,
      auth,
      ...(hostKey ? { hostKey: { type: hostKey.type, key: hostKey.key } } : {}),
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

  // 编辑：密码登录时不填密码 = 保留原来的；填了 = 换掉；改回密钥登录 = 删掉存着的密码和主机公钥。
  // 主机公钥同理：没带 = 沿用确认过的那把（地址没变时），带了 = 用户重新确认的那把。
  // 地址、端口、登录方式、密码或主机公钥变了，之前的连通结果和探测到的系统都不再算数。
  async update(id, input) {
    const machine = this.machines.get(id)
    if (!machine) throw new ExecutorError('machine_not_found', 'Machine was removed.')
    const connection = normalizeSshConnection({
      target: input?.target ?? machine.target,
      port: input?.port === undefined ? machine.port : input.port,
    })
    const auth = input?.auth === undefined ? machine.auth : normalizeSshAuth(input.auth)
    const password = auth === 'password' ? normalizeSshPassword(input?.password) : ''
    if (
      auth === 'password' &&
      !password &&
      !(this.#sameAddress(id, connection) && this.secrets.has(id))
    )
      throw this.#passwordAgain(id)
    const hostKey = auth === 'password' ? this.#confirmedHostKey(input, id, connection) : null
    if (auth === 'password' && !password && !this.#reusesPassword(id, connection, hostKey))
      throw this.#passwordAgain(id)
    const changed =
      connection.target !== machine.target ||
      connection.port !== machine.port ||
      auth !== machine.auth ||
      Boolean(password) ||
      (Boolean(hostKey) && !sameHostKey(hostKey, machine.hostKey))
    if (auth === 'password' && password) await this.secrets.set(id, password)
    if (auth === 'key') await this.secrets.delete(id)
    Object.assign(machine, connection, { auth })
    if (hostKey) machine.hostKey = { type: hostKey.type, key: hostKey.key }
    else delete machine.hostKey
    if (input?.name !== undefined)
      machine.name = cleanText(input.name, MAX_NAME_CHARS) || connection.target
    if (input?.defaultCwd !== undefined)
      machine.defaultCwd = cleanText(input.defaultCwd, MAX_PATH_CHARS)
    if (changed) {
      machine.lastCheck = null
      delete machine.system
      this.authRefused.delete(id)
    }
    await this.persist()
    return this.publicMachine(machine)
  }

  async remove(id) {
    this.authRefused.delete(id)
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
    // 用户点的测试：放开一次，让这次真的去连；再被拒绝就又记上。
    this.authRefused.delete(id)
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
