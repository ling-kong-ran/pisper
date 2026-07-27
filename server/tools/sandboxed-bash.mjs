import { createBashTool } from '@earendil-works/pi-coding-agent'
import { Type } from 'typebox'
import { applyWindowsUtf8Environment } from './windows-utf8-bash.mjs'

const sandboxedBashSchema = Type.Object({
  command: Type.String({ description: 'Shell command to execute' }),
  timeout: Type.Optional(Type.Number({ description: 'Timeout in seconds' })),
  sandbox_permissions: Type.Optional(Type.String({
    enum: ['workspace', 'require_escalated'],
    description: 'Use workspace for normal sandboxed execution. Use require_escalated only when the command must access resources blocked by the workspace sandbox.',
  })),
  justification: Type.Optional(Type.String({ description: 'Short user-facing reason why this command needs to run outside the workspace sandbox' })),
})

export function shouldRunBashOutsideSandbox(mode, params = {}) {
  return mode === 'full-access' || params.sandbox_permissions === 'require_escalated'
}

export function createPisperBashTool(cwd, { sandboxService, getExecutionMode, platform = process.platform } = {}) {
  const localTool = createBashTool(cwd, {
    spawnHook: (context) => applyWindowsUtf8Environment(context, platform),
  })
  const sandboxedTool = createBashTool(cwd, {
    operations: sandboxService.createBashOperations(),
  })
  return {
    ...localTool,
    description: `${localTool.description}\nCommands run inside the workspace sandbox by default. If a necessary command is blocked because it needs access outside the workspace or to a blocked network destination, retry with sandbox_permissions="require_escalated" and provide a concise justification. Do not request escalation for ordinary project edits, builds, or tests.`,
    parameters: sandboxedBashSchema,
    async execute(id, params, signal, onUpdate) {
      const mode = getExecutionMode?.() || 'workspace'
      const target = shouldRunBashOutsideSandbox(mode, params) ? localTool : sandboxedTool
      return target.execute(id, { command: params.command, timeout: params.timeout }, signal, onUpdate)
    },
  }
}
