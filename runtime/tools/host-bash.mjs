// 宿主 bash 工具：基于 Pi 引擎 createBashTool 的封装，
// 附带命令守卫与 Windows UTF-8 环境修正，并拒绝危险的敏感环境变量覆盖。
import { createBashTool } from '../runtime/pi-coding-agent.mjs'
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

export function hostCommandEnvironment(environment = {}) {
  const result = {}
  for (const [name, value] of Object.entries(environment)) {
    if (DENIED_ENVIRONMENT_NAMES.has(name) || CREDENTIAL_ENVIRONMENT_NAME.test(name)) continue
    result[name] = value
  }
  return result
}

export async function createPisperBashTool(cwd, { platform = process.platform } = {}) {
  const localTool = await createBashTool(cwd, {
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
  return {
    ...localTool,
    description: `${localTool.description}\nCommands run as the current operating-system user and can access host files and networks.`,
    parameters: HOST_BASH_SCHEMA,
  }
}
