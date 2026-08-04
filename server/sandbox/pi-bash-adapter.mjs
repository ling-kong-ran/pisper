import { isAbsolute, relative, resolve } from 'node:path'
import { PISPER_SANDBOX_LIMITS, PISPER_SANDBOX_SETTABLE_ENVIRONMENT } from './pisper-profiles.mjs'

const SETTABLE_ENVIRONMENT = new Set(PISPER_SANDBOX_SETTABLE_ENVIRONMENT)

export function sandboxExecutionEnvironment(environment = {}) {
  return Object.fromEntries(
    Object.entries(environment).filter(
      ([name, value]) => SETTABLE_ENVIRONMENT.has(name) && value != null,
    ),
  )
}

export function sandboxLogicalCwd(workspace, cwd) {
  const root = resolve(workspace)
  const target = resolve(cwd || workspace)
  const path = relative(root, target)
  if (
    path === '..' ||
    path.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) ||
    isAbsolute(path)
  ) {
    throw new Error('Sandbox command cwd is outside the workspace.')
  }
  return { mount: 'workspace', path: path || '.' }
}

export function createSandboxBashOperations({ workspace, getSandbox }) {
  if (typeof getSandbox !== 'function') throw new Error('Sandbox handle provider is required.')
  return {
    async exec(command, cwd, { onData, signal, timeout, env } = {}) {
      const sandbox = await getSandbox()
      const wallTimeMs = timeout
        ? Math.min(PISPER_SANDBOX_LIMITS.wallTimeMs, Math.max(1, timeout * 1000))
        : undefined
      const result = await sandbox.exec({
        command: { kind: 'shell', shell: 'default', script: command },
        cwd: sandboxLogicalCwd(workspace, cwd),
        env: sandboxExecutionEnvironment(env),
        signal,
        limits: wallTimeMs ? { wallTimeMs } : {},
        onOutput: ({ bytes }) => onData?.(Buffer.from(bytes)),
      })
      if (signal?.aborted || result.signal === 'terminate' || result.signal === 'kill') {
        throw new Error('aborted')
      }
      if (result.signal === 'timeout') throw new Error(`timeout:${timeout || wallTimeMs / 1000}`)
      if (result.signal) throw new Error(`Sandbox command terminated: ${result.signal}`)
      return { exitCode: result.exitCode }
    },
  }
}
