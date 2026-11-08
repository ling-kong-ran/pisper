// SSH 机器的登记表：只存怎么连（地址、端口、默认目录），不存密码和私钥，凭据全部留给系统的 ssh。
import { randomBytes } from 'node:crypto'
import { join } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import { ExecutorError } from './bots/executor.mjs'
import { SshExecutor } from './ssh-executor.mjs'

const STORE_VERSION = 1
const MAX_MACHINES = 32
const MAX_NAME_CHARS = 40
const MAX_TARGET_CHARS = 255
const MAX_PATH_CHARS = 1024
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

function normalizeStored(value) {
  if (!value || typeof value !== 'object' || !isSshMachineId(value.id)) return null
  try {
    const connection = normalizeSshConnection(value)
    return {
      id: value.id,
      name: cleanText(value.name, MAX_NAME_CHARS) || connection.target,
      ...connection,
      defaultCwd: cleanText(value.defaultCwd, MAX_PATH_CHARS),
      addedAt: String(value.addedAt || ''),
      lastCheck:
        value.lastCheck && typeof value.lastCheck === 'object'
          ? {
              ok: value.lastCheck.ok === true,
              at: String(value.lastCheck.at || ''),
              code: cleanText(value.lastCheck.code, 40),
              platform: cleanText(value.lastCheck.platform, 40),
            }
          : null,
    }
  } catch {
    return null
  }
}

export class SshMachineService {
  constructor({ path, dataDir, extraArguments = [], now = () => new Date().toISOString() }) {
    this.path = path
    this.muxDirectory = join(dataDir, 'ssh-mux')
    this.extraArguments = extraArguments
    this.now = now
    this.machines = new Map()
    this.write = Promise.resolve()
  }

  async init() {
    const stored = await readJson(this.path, { version: STORE_VERSION, machines: [] })
    this.machines.clear()
    for (const raw of Array.isArray(stored?.machines) ? stored.machines : []) {
      const machine = normalizeStored(raw)
      if (machine) this.machines.set(machine.id, machine)
    }
  }

  persist() {
    const snapshot = { version: STORE_VERSION, machines: [...this.machines.values()] }
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  list() {
    return [...this.machines.values()].map((machine) => ({ ...machine }))
  }

  get(id) {
    return this.machines.get(id) || null
  }

  // 机器被移除后还有会话指着它：执行器照常给，第一个操作会以“已移除”失败。
  executor(id) {
    const machine = this.machines.get(id)
    if (!machine) return new RemovedMachineExecutor(id)
    return this.executorFor(machine, () => this.machines.has(id))
  }

  executorFor(machine, isAvailable) {
    return new SshExecutor({
      machine,
      muxDirectory: this.muxDirectory,
      extraArguments: this.extraArguments,
      ...(isAvailable ? { isAvailable } : {}),
    })
  }

  // 还没保存的连接也能测：添加表单在保存前就给出结果。
  async test(input) {
    const connection = normalizeSshConnection(input)
    const executor = this.executorFor({
      id: 'ssh_unsaved0',
      name: connection.target,
      ...connection,
    })
    return executor.probe()
  }

  async add(input) {
    const connection = normalizeSshConnection(input)
    if (this.machines.size >= MAX_MACHINES)
      throw new ExecutorError('too_many_machines', `Machine limit reached: max ${MAX_MACHINES}.`)
    const machine = {
      id: `ssh_${randomBytes(9).toString('base64url')}`,
      name: cleanText(input?.name, MAX_NAME_CHARS) || connection.target,
      ...connection,
      defaultCwd: cleanText(input?.defaultCwd, MAX_PATH_CHARS),
      addedAt: this.now(),
      lastCheck: null,
    }
    this.machines.set(machine.id, machine)
    await this.persist()
    return { ...machine }
  }

  async remove(id) {
    if (!this.machines.delete(id)) return false
    await this.persist()
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

  // 实际连上过（选目录、建会话）：列表里的状态跟着更新，不要求用户再点一次测试。
  async recordReachable(id) {
    const machine = this.machines.get(id)
    if (machine?.lastCheck?.ok) return
    await this.recordCheck(id, { ok: true, platform: machine?.lastCheck?.platform || '' })
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
