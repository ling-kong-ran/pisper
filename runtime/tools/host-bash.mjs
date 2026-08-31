// 宿主 bash 工具：基于 Pi 引擎 createBashTool 的封装，
// 附带命令守卫与 Windows UTF-8 环境修正，并拒绝危险的敏感环境变量覆盖。
import { spawn } from 'node:child_process'
import { createBashTool, getShellConfig } from '../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { applyWindowsUtf8Environment } from './windows-utf8-bash.mjs'
import { formatGuardError, guardCommand } from './command-guard.mjs'

const HOST_BASH_SCHEMA = Type.Object({
  command: Type.String({ description: 'Shell command to execute' }),
  timeout: Type.Optional(Type.Number({ description: 'Timeout in seconds' })),
})

const DENIED_ENVIRONMENT_NAMES = new Set([
  'ANTHROPIC_API_KEY',
  'OPENAI_API_KEY',
  'GOOGLE_API_KEY',
  'GEMINI_API_KEY',
  'XAI_API_KEY',
  'OPENROUTER_API_KEY',
  'GITHUB_TOKEN',
  'GH_TOKEN',
  'GITLAB_TOKEN',
  'BITBUCKET_TOKEN',
  'NPM_TOKEN',
  'NODE_AUTH_TOKEN',
  'HF_TOKEN',
  'HUGGING_FACE_HUB_TOKEN',
  'DOCKER_AUTH_CONFIG',
  'DATABASE_URL',
  'AWS_ACCESS_KEY_ID',
  'AWS_SECRET_ACCESS_KEY',
  'AWS_SESSION_TOKEN',
  'AZURE_CLIENT_SECRET',
  'GOOGLE_APPLICATION_CREDENTIALS',
  'BASH_ENV',
  'ENV',
  'PROMPT_COMMAND',
  'SHELLOPTS',
  'BASHOPTS',
  'CDPATH',
  'GLOBIGNORE',
])

const CREDENTIAL_ENVIRONMENT_NAME =
  /(?:^|_)(?:API_?KEY|ACCESS_?TOKEN|AUTH_?TOKEN|SECRET|PASSWORD|PASSWD|PRIVATE_?KEY)$/i
const MAX_TIMEOUT_MS = 2_147_483_647
const MAX_TIMEOUT_SECONDS = MAX_TIMEOUT_MS / 1000

export function hostCommandEnvironment(environment = {}) {
  const result = {}
  for (const [name, value] of Object.entries(environment)) {
    if (DENIED_ENVIRONMENT_NAMES.has(name) || CREDENTIAL_ENVIRONMENT_NAME.test(name)) continue
    result[name] = value
  }
  return result
}

export function windowsPowerShellExecutable(environment = process.env) {
  const systemRoot = environment.SystemRoot || environment.windir || 'C:\\Windows'
  return `${systemRoot}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe`
}

export function windowsPowerShellArguments(command) {
  return ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', command]
}

function resolveTimeoutMs(timeout) {
  if (timeout === undefined) return undefined
  if (!Number.isFinite(timeout) || timeout <= 0) {
    throw new Error('Invalid timeout: must be a finite number of seconds')
  }
  const timeoutMs = timeout * 1000
  if (timeoutMs > MAX_TIMEOUT_MS) {
    throw new Error(`Invalid timeout: maximum is ${MAX_TIMEOUT_SECONDS} seconds`)
  }
  return timeoutMs
}

// Bash 不存在时使用系统 PowerShell；保持同一套流式输出与中止/超时协议，避免模型收到假成功。
export function createWindowsSystemShellOperations(environment = process.env) {
  const shell = windowsPowerShellExecutable(environment)
  return {
    exec: async (command, cwd, { onData, signal, timeout, env }) => {
      const timeoutMs = resolveTimeoutMs(timeout)
      if (signal?.aborted) throw new Error('aborted')
      return await new Promise((resolve, reject) => {
        const child = spawn(shell, windowsPowerShellArguments(command), {
          cwd,
          env,
          stdio: ['ignore', 'pipe', 'pipe'],
          windowsHide: true,
        })
        let settled = false
        let timeoutHandle
        const cleanup = () => {
          if (timeoutHandle) clearTimeout(timeoutHandle)
          signal?.removeEventListener('abort', abort)
        }
        const settle = (callback, value) => {
          if (settled) return
          settled = true
          cleanup()
          callback(value)
        }
        const abort = () => {
          child.kill()
          settle(reject, new Error('aborted'))
        }
        const killForTimeout = () => {
          child.kill()
          settle(reject, new Error(`timeout:${timeout}`))
        }
        child.stdout?.on('data', onData)
        child.stderr?.on('data', onData)
        child.once('error', (error) => settle(reject, error))
        child.once('close', (exitCode) => settle(resolve, { exitCode }))
        if (timeoutMs !== undefined) timeoutHandle = setTimeout(killForTimeout, timeoutMs)
        if (signal) signal.addEventListener('abort', abort, { once: true })
        if (signal?.aborted) abort()
      })
    },
  }
}

export async function selectHostShell(platform, shellConfig = getShellConfig) {
  if (platform !== 'win32') return { fallback: false }
  try {
    await shellConfig()
    return { fallback: false }
  } catch {
    return { fallback: true }
  }
}

export async function createPisperBashTool(
  cwd,
  { platform = process.platform, shellConfig = getShellConfig, environment = process.env } = {},
) {
  const selectedShell = await selectHostShell(platform, shellConfig)
  const localTool = await createBashTool(cwd, {
    ...(selectedShell.fallback
      ? { operations: createWindowsSystemShellOperations(environment) }
      : {}),
    spawnHook: (context) => {
      const decision = guardCommand(context.command, { platform })
      if (decision.blocked && decision.severity === 'block') {
        throw new Error(formatGuardError(decision, context.command))
      }
      return applyWindowsUtf8Environment(
        {
          ...context,
          env: hostCommandEnvironment(context.env),
        },
        platform,
      )
    },
  })
  const shellNote = selectedShell.fallback
    ? 'Bash was not found on Windows, so commands run with the system Windows PowerShell (powershell.exe).'
    : 'Commands run as the current operating-system user and can access host files and networks.'
  const description = selectedShell.fallback
    ? localTool.description.replace('Execute a bash command', 'Execute a command')
    : localTool.description
  return {
    ...localTool,
    description: `${description}\n${shellNote}`,
    promptSnippet: selectedShell.fallback
      ? 'Execute commands with Windows PowerShell'
      : localTool.promptSnippet,
    parameters: HOST_BASH_SCHEMA,
  }
}
