// 远程机器的接口：SSH 机器和让对方连过来的机器（执行节点）。外部响应一律先归一化。
import { ApiError, requestJson } from '@/lib/http'

export type MachineKind = 'ssh' | 'node'

export type MachineCheck = { ok: boolean; at: string; code: string; platform: string }

export type MachineAuth = 'key' | 'password'

// 连上后探测到的系统（SSH）或执行节点报告的系统；还没连过时为 null。
export type MachineSystem = { os: string; family: 'posix' | 'windows'; name: string; shell: string }

export type Machine = {
  id: string
  kind: MachineKind
  name: string
  target: string
  port: number | null
  // 密码本身永远不会回到前端，只知道设没设。
  auth: MachineAuth
  passwordSet: boolean
  defaultCwd: string
  online: boolean
  lastCheck: MachineCheck | null
  system: MachineSystem | null
  roots: string[]
}

export type MachineRemoteInfo = {
  enabled: boolean
  endpoints: string[]
  fingerprint: string | null
}

export type MachinePairing = { code: string; expiresAt: string; remote: MachineRemoteInfo }

export type SshTestResult =
  { ok: true; platform: string; home: string } | { ok: false; code: string; message: string }

type Fields = Record<string, unknown>

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

function record(value: unknown): Fields {
  return value && typeof value === 'object' ? (value as Fields) : {}
}

function parseSystem(value: unknown): MachineSystem | null {
  const input = record(value)
  if (!text(input.os)) return null
  return {
    os: text(input.os),
    family: input.family === 'windows' ? 'windows' : 'posix',
    name: text(input.name),
    shell: text(input.shell),
  }
}

export function parseMachine(value: unknown): Machine {
  const input = record(value)
  const check = input.lastCheck ? record(input.lastCheck) : null
  return {
    id: text(input.id),
    kind: input.kind === 'node' ? 'node' : 'ssh',
    name: text(input.name),
    target: text(input.target),
    port: typeof input.port === 'number' ? input.port : null,
    auth: input.auth === 'password' ? 'password' : 'key',
    passwordSet: input.passwordSet === true,
    defaultCwd: text(input.defaultCwd),
    online: input.online === true,
    lastCheck: check
      ? {
          ok: check.ok === true,
          at: text(check.at),
          code: text(check.code),
          platform: text(check.platform),
        }
      : null,
    system: parseSystem(input.system),
    roots: Array.isArray(input.roots) ? input.roots.map(text).filter(Boolean) : [],
  }
}

function parseRemote(value: unknown): MachineRemoteInfo {
  const input = record(value)
  return {
    enabled: input.enabled === true,
    endpoints: Array.isArray(input.endpoints) ? input.endpoints.map(text).filter(Boolean) : [],
    fingerprint: text(input.fingerprint) || null,
  }
}

export function fetchMachines(signal?: AbortSignal) {
  return requestJson('/api/machines', {
    signal,
    parse: (value) => {
      const input = record(value)
      return {
        machines: (Array.isArray(input.machines) ? input.machines : [])
          .map(parseMachine)
          .filter((machine) => machine.id),
        remote: parseRemote(input.remote),
      }
    },
  })
}

export type SshMachineInput = {
  name?: string
  target: string
  port?: number | null
  defaultCwd?: string
  auth?: MachineAuth
  // 只在新填或要换掉时发送；编辑时不填表示保留已存的密码，改成密钥登录会删掉它。
  password?: string
}

export function createSshMachine(input: SshMachineInput) {
  return requestJson('/api/machines/ssh', { method: 'POST', data: input, parse: parseMachine })
}

export function updateSshMachine(id: string, input: SshMachineInput) {
  return requestJson(`/api/machines/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    data: input,
    parse: parseMachine,
  })
}

export function parseSshTestResult(value: unknown): SshTestResult {
  const input = record(value)
  return input.ok === true
    ? { ok: true, platform: text(input.platform), home: text(input.home) }
    : { ok: false, code: text(input.code), message: text(input.message) }
}

// 编辑已有机器时带上 id：没重新输入密码就用存着的那个测试。
export function testSshConnection(
  input: Pick<SshMachineInput, 'target' | 'port' | 'auth' | 'password'> & { id?: string },
) {
  return requestJson('/api/machines/ssh/test', {
    method: 'POST',
    data: input,
    // 对方慢的时候要等 ssh 自己超时（10 秒）再加上跑探测脚本。
    timeout: 40_000,
    parse: parseSshTestResult,
  })
}

export function checkMachine(id: string) {
  return requestJson(`/api/machines/${encodeURIComponent(id)}/check`, {
    method: 'POST',
    timeout: 40_000,
    parse: parseSshTestResult,
  })
}

export function removeMachine(id: string) {
  return requestJson(`/api/machines/${encodeURIComponent(id)}`, { method: 'DELETE' })
}

export function createMachinePairing() {
  return requestJson('/api/machines/pairing', {
    method: 'POST',
    parse: (value): MachinePairing => {
      const input = record(value)
      const pairing = record(input.pairing)
      return {
        code: text(pairing.code),
        expiresAt: text(pairing.expiresAt),
        remote: parseRemote(input.remote),
      }
    },
  })
}

// 选目录时就连过去确认：~ 换成绝对路径，目录不存在、连不上都在这一步说清楚。
export function resolveMachineFolder(id: string, cwd: string, signal?: AbortSignal) {
  return requestJson(`/api/machines/${encodeURIComponent(id)}/folder`, {
    method: 'POST',
    data: { cwd },
    timeout: 40_000,
    signal,
    parse: (value) => text(record(value).cwd),
  })
}

export function machineErrorCode(error: unknown) {
  return error instanceof ApiError ? text(record(error.data).code) : ''
}
