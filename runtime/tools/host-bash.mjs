// 宿主 bash 工具：基于 Pi 引擎 createBashTool 的封装，
// 附带命令守卫与 Windows UTF-8 环境修正，并拒绝危险的敏感环境变量覆盖。
import {
  createBashTool,
  createLocalShellOperations,
  getShellConfig,
} from '../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { applyWindowsUtf8Environment } from './windows-utf8-bash.mjs'
import { formatGuardError, guardCommand } from './command-guard.mjs'
import { leanToolDefinition } from './lean-coding-tools.mjs'
import { hostBashDescription } from '../prompts/tools/host-bash.mjs'
import { LEAN_TOOL_TEXT } from '../prompts/tools/lean-coding-tools.mjs'

const HOST_BASH_SCHEMA = Type.Object({
  command: Type.String({ description: LEAN_TOOL_TEXT.bash.parameters.command }),
  timeout: Type.Optional(Type.Number({ description: LEAN_TOOL_TEXT.bash.parameters.timeout })),
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

// 与 Bash 共用 Pi 的进程生命周期：取消/超时必须终止整个进程树，
// 否则 PowerShell 退出后 Python 等子进程仍会运行并继续占用内存。
export function createWindowsSystemShellOperations(environment = process.env) {
  return createLocalShellOperations('PowerShell', () => ({
    shell: windowsPowerShellExecutable(environment),
    args: windowsPowerShellArguments('').slice(0, -1),
  }))
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
      ? { operations: await createWindowsSystemShellOperations(environment) }
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
  const leanTool = leanToolDefinition(localTool)
  return {
    ...leanTool,
    description: hostBashDescription(leanTool.description, { powershell: selectedShell.fallback }),
    parameters: HOST_BASH_SCHEMA,
  }
}
