import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import test from 'node:test'
import {
  DEFAULT_SANDBOX_DOMAINS,
  buildCommandSandboxOverrides,
  buildSandboxConfig,
  canonicalSandboxPath,
  packagedSandboxExecutablePath,
  sandboxChildEnvironment,
  windowsSensitiveGuardPaths,
} from '../security/local-sandbox-policy.mjs'
import { LocalSandboxService } from '../services/local-sandbox-service.mjs'
import { shouldRunBashOutsideSandbox } from '../tools/sandboxed-bash.mjs'

const TEST_WINDOWS_SHELL = 'C:\\Program Files\\Git\\bin\\bash.exe'
const TEST_WINDOWS_HOME = resolve(join(tmpdir(), 'pisper-sandbox-empty-home'))

test('sandbox config accepts workspace sets and protects credentials', () => {
  const workspace = resolve('workspace')
  const dataDir = resolve('private-agent-data')
  const config = buildSandboxConfig({ workspaces: new Set([workspace]), dataDir, platform: 'linux', homeDir: resolve('sandbox-home') })

  assert.deepEqual(config.filesystem.allowRead, [])
  assert.deepEqual(config.filesystem.allowWrite, [workspace])
  assert.ok(config.filesystem.denyRead.includes(dataDir))
  assert.ok(config.filesystem.denyRead.some((path) => path.endsWith('.ssh')))
  assert.ok(config.credentials.envVars.some((entry) => entry.name === 'OPENAI_API_KEY' && entry.mode === 'deny'))
  assert.equal(DEFAULT_SANDBOX_DOMAINS.includes('localhost'), false)
  assert.equal(config.network.strictAllowlist, true)
})

test('packagedSandboxExecutablePath remaps packed app.asar paths but never double-unpacks', () => {
  const win = (segments) => segments.join('\\')
  const packed = win(['C:', 'app', 'resources', 'app.asar', 'node_modules', '@anthropic-ai', 'sandbox-runtime', 'vendor', 'srt-win', 'x64', 'srt-win.exe'])
  const unpacked = win(['C:', 'app', 'resources', 'app.asar.unpacked', 'node_modules', '@anthropic-ai', 'sandbox-runtime', 'vendor', 'srt-win', 'x64', 'srt-win.exe'])
  const dev = win(['E:', 'code', 'pi-coder', 'node_modules', '@anthropic-ai', 'sandbox-runtime', 'vendor', 'srt-win', 'x64', 'srt-win.exe'])

  // Packed archive path is remapped to its unpacked counterpart.
  assert.equal(packagedSandboxExecutablePath(packed), unpacked)
  // An already-unpacked path must be returned verbatim - the old
  // implementation produced 'app.asar.unpacked.unpacked' here.
  assert.equal(packagedSandboxExecutablePath(unpacked), unpacked)
  // Paths outside any asar are untouched.
  assert.equal(packagedSandboxExecutablePath(dev), dev)
  // The function is idempotent even when applied repeatedly.
  assert.equal(packagedSandboxExecutablePath(packagedSandboxExecutablePath(unpacked)), unpacked)
  // Empty / missing input is safe.
  assert.equal(packagedSandboxExecutablePath(''), '')
  assert.equal(packagedSandboxExecutablePath(undefined), '')
})

test('POSIX commands deny writes to other registered workspaces without Windows ACL stamps', () => {
  const first = resolve('workspace-a')
  const second = resolve('workspace-b')
  assert.deepEqual(buildCommandSandboxOverrides(new Set([first, second]), first, 'linux'), {
    filesystem: { denyWrite: [second] },
  })
  assert.equal(buildCommandSandboxOverrides(new Set([first, second]), first, 'win32'), undefined)
})

test('Windows config avoids large home/workspace deny stamps and protects bounded sensitive paths', () => {
  const workspace = resolve('workspace')
  const dataDir = resolve(tmpdir())
  const homeDir = resolve('sandbox-home')
  const config = buildSandboxConfig({ workspaces: new Set([workspace]), dataDir, platform: 'win32', homeDir })
  assert.deepEqual(config.filesystem.allowWrite, [workspace])
  assert.deepEqual(config.filesystem.allowRead, [])
  assert.deepEqual(config.filesystem.denyRead, [canonicalSandboxPath(dataDir)])
  assert.deepEqual(config.filesystem.denyWrite, [])
  assert.throws(
    () => buildSandboxConfig({ workspaces: new Set([workspace]), dataDir: join(workspace, 'agent-data'), platform: 'win32', homeDir }),
    /不能授权包含敏感目录的路径/,
  )
  assert.throws(
    () => buildSandboxConfig({ workspaces: new Set(), dataDir: resolve('private-agent-data'), platform: 'win32', homeDir }),
    /必须且只能授权一个工作区/,
  )
  assert.throws(
    () => buildSandboxConfig({ workspaces: new Set([workspace, resolve('workspace-b')]), dataDir: resolve('private-agent-data'), platform: 'win32', homeDir }),
    /必须且只能授权一个工作区/,
  )
})

test('Windows command preflight guards readable credential paths before user code runs', async (t) => {
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-guard-workspace-'))
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-guard-data-'))
  const homeDir = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-guard-home-'))
  await writeFile(join(homeDir, '.git-credentials'), 'https://user:secret@example.test\n', 'utf8')
  t.after(() => Promise.all([
    rm(workspace, { recursive: true, force: true }),
    rm(dataDir, { recursive: true, force: true }),
    rm(homeDir, { recursive: true, force: true }),
  ]))
  let wrappedCommand = ''
  let initializedConfig
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => false,
    initialize: async (config) => { initializedConfig = config },
    wrapWithSandboxArgv: async (command) => {
      wrappedCommand = command
      return { argv: [process.execPath, '-e', ''], env: {} }
    },
    cleanupAfterCommand: () => {},
    reset: async () => {},
  }
  const service = new LocalSandboxService({ dataDir, homeDir, platform: 'win32', manager, windowsShell: TEST_WINDOWS_SHELL })

  await service.createBashOperations().exec('printf USER_COMMAND', workspace, { onData: () => {}, timeout: 5 })

  assert.deepEqual(initializedConfig.filesystem.denyRead, [canonicalSandboxPath(dataDir)])
  const guardedPaths = windowsSensitiveGuardPaths(dataDir, homeDir)
  assert.ok(guardedPaths.includes(canonicalSandboxPath(dataDir)))
  assert.ok(guardedPaths.includes(canonicalSandboxPath(resolve(homeDir, '.git-credentials'))))
  assert.match(wrappedCommand, /PISPER_SANDBOX_SENSITIVE_PATH_READABLE/)
  assert.match(wrappedCommand, /printf USER_COMMAND/)
  await service.dispose()
})

test('new credential files rotate the Windows sandbox policy before the next command', async (t) => {
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-rotate-workspace-'))
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-rotate-data-'))
  const homeDir = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-rotate-home-'))
  t.after(() => Promise.all([
    rm(workspace, { recursive: true, force: true }),
    rm(dataDir, { recursive: true, force: true }),
    rm(homeDir, { recursive: true, force: true }),
  ]))
  let enabled = false
  let resetCount = 0
  const configs = []
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => enabled,
    initialize: async (config) => { enabled = true; configs.push(config) },
    wrapWithSandboxArgv: async () => ({ argv: [process.execPath, '-e', ''], env: {} }),
    cleanupAfterCommand: () => {},
    reset: async () => { enabled = false; resetCount += 1 },
  }
  const service = new LocalSandboxService({ dataDir, homeDir, platform: 'win32', manager, windowsShell: TEST_WINDOWS_SHELL })
  const operations = service.createBashOperations()

  await operations.exec('first', workspace, { onData: () => {}, timeout: 5 })
  const sshDir = join(homeDir, '.ssh')
  await mkdir(sshDir)
  const credentialFile = join(sshDir, 'id_test')
  await writeFile(credentialFile, 'private', 'utf8')
  await operations.exec('second', workspace, { onData: () => {}, timeout: 5 })

  assert.equal(resetCount, 1)
  assert.equal(configs.length, 2)
  assert.ok(configs[1].filesystem.denyRead.includes(canonicalSandboxPath(credentialFile)))
  await service.dispose()
})

test('sandbox initialization failures fail closed before spawning a command', async () => {
  let wrapped = false
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => false,
    initialize: async () => { throw new Error('sandbox setup failed') },
    wrapWithSandboxArgv: async () => { wrapped = true },
    cleanupAfterCommand: () => {},
    reset: async () => {},
  }
  const service = new LocalSandboxService({ dataDir: resolve('sandbox-data'), platform: 'linux', manager })

  await assert.rejects(
    service.createBashOperations().exec('echo unsafe', process.cwd(), { onData: () => {}, timeout: 1 }),
    /sandbox setup failed/,
  )
  assert.equal(wrapped, false)
  assert.equal(service.activeCommands, 0)
})

test('sandbox operations spawn only the wrapper-owned environment', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-sandbox-op-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let initializedConfig
  let wrappedConfig
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => false,
    initialize: async (config) => { initializedConfig = config },
    wrapWithSandboxArgv: async (_command, _shell, config) => {
      wrappedConfig = config
      return {
        argv: [process.execPath, '-e', 'process.stdout.write(process.env.SANDBOX_MARKER || "missing")'],
        env: { SANDBOX_MARKER: 'sandbox-owned', BASH_ENV: resolve('untrusted-bash-env') },
      }
    },
    cleanupAfterCommand: () => {},
    reset: async () => {},
  }
  const service = new LocalSandboxService({ dataDir: join(directory, 'data'), platform: 'linux', manager })
  let output = ''

  const result = await service.createBashOperations().exec('ignored by fake wrapper', directory, {
    onData: (chunk) => { output += chunk.toString() },
    timeout: 5,
  })

  assert.equal(result.exitCode, 0)
  assert.equal(output, 'sandbox-owned')
  assert.equal(sandboxChildEnvironment({ SAFE: 'yes', BASH_ENV: 'unsafe', PROMPT_COMMAND: 'unsafe' }).SAFE, 'yes')
  assert.equal('BASH_ENV' in sandboxChildEnvironment({ BASH_ENV: 'unsafe' }), false)
  assert.equal('PROMPT_COMMAND' in sandboxChildEnvironment({ PROMPT_COMMAND: 'unsafe' }), false)
  assert.deepEqual(initializedConfig.filesystem.allowWrite, [canonicalSandboxPath(directory)])
  assert.deepEqual(wrappedConfig, { filesystem: { denyWrite: [] } })
  await service.dispose()
})

test('Windows switches one granted workspace at a time instead of accumulating roots', async (t) => {
  const first = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-a-'))
  const second = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-b-'))
  t.after(() => Promise.all([
    rm(first, { recursive: true, force: true }),
    rm(second, { recursive: true, force: true }),
  ]))
  let enabled = false
  let resetCount = 0
  const initialized = []
  const wrappedConfigs = []
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => enabled,
    initialize: async (config) => { enabled = true; initialized.push(config) },
    wrapWithSandboxArgv: async (_command, _shell, config) => {
      wrappedConfigs.push(config)
      return { argv: [process.execPath, '-e', ''], env: {} }
    },
    cleanupAfterCommand: () => {},
    reset: async () => { enabled = false; resetCount += 1 },
  }
  const service = new LocalSandboxService({ dataDir: join(tmpdir(), 'pisper-sandbox-win-data'), homeDir: TEST_WINDOWS_HOME, platform: 'win32', manager, windowsShell: TEST_WINDOWS_SHELL })
  const operations = service.createBashOperations()

  await operations.exec('first', first, { onData: () => {}, timeout: 5 })
  await operations.exec('second', second, { onData: () => {}, timeout: 5 })

  assert.deepEqual(initialized.map((config) => config.filesystem.allowWrite), [
    [canonicalSandboxPath(first)],
    [canonicalSandboxPath(second)],
  ])
  assert.deepEqual(wrappedConfigs, [undefined, undefined])
  assert.equal(resetCount, 1)
  await service.dispose()
})

test('same Windows workspace commands run concurrently without reinitializing SRT', async (t) => {
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-shared-'))
  t.after(() => rm(workspace, { recursive: true, force: true }))
  let enabled = false
  let initializeCount = 0
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => enabled,
    initialize: async () => { enabled = true; initializeCount += 1 },
    wrapWithSandboxArgv: async () => ({
      argv: [process.execPath, '-e', 'setTimeout(() => {}, 180)'],
      env: {},
    }),
    cleanupAfterCommand: () => {},
    reset: async () => { enabled = false },
  }
  const service = new LocalSandboxService({ dataDir: join(tmpdir(), 'pisper-sandbox-shared-data'), homeDir: TEST_WINDOWS_HOME, platform: 'win32', manager, windowsShell: TEST_WINDOWS_SHELL })
  const operations = service.createBashOperations()

  const first = operations.exec('first', workspace, { onData: () => {}, timeout: 5 })
  const second = operations.exec('second', workspace, { onData: () => {}, timeout: 5 })
  for (let attempt = 0; attempt < 40 && service.activeCommands < 2; attempt += 1) await delay(10)

  assert.equal(service.activeCommands, 2)
  assert.equal(initializeCount, 1)
  assert.deepEqual(service.schedulerStatus(), {
    activeCommands: 2,
    pendingCommands: 0,
    switchingWorkspace: false,
    lastSwitchDurationMs: service.lastSwitchDurationMs,
  })
  await Promise.all([first, second])
  await service.dispose()
})

test('an aborted cross-workspace waiter never resets or switches the active Windows sandbox', async (t) => {
  const first = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-abort-a-'))
  const second = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-abort-b-'))
  t.after(() => Promise.all([
    rm(first, { recursive: true, force: true }),
    rm(second, { recursive: true, force: true }),
  ]))
  let enabled = false
  let resetCount = 0
  const initialized = []
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => enabled,
    initialize: async (config) => { enabled = true; initialized.push(config.filesystem.allowWrite) },
    wrapWithSandboxArgv: async () => ({ argv: [process.execPath, '-e', 'setTimeout(() => {}, 250)'], env: {} }),
    cleanupAfterCommand: () => {},
    reset: async () => { enabled = false; resetCount += 1 },
  }
  const service = new LocalSandboxService({ dataDir: join(tmpdir(), 'pisper-sandbox-abort-data'), homeDir: TEST_WINDOWS_HOME, platform: 'win32', manager, windowsShell: TEST_WINDOWS_SHELL })
  const operations = service.createBashOperations()
  const running = operations.exec('running', first, { onData: () => {}, timeout: 5 })
  for (let attempt = 0; attempt < 40 && service.activeCommands < 1; attempt += 1) await delay(10)

  const controller = new AbortController()
  const waiting = operations.exec('waiting', second, { onData: () => {}, signal: controller.signal, timeout: 5 })
  for (let attempt = 0; attempt < 40 && !service.switchingWorkspace; attempt += 1) await delay(10)
  controller.abort()

  await assert.rejects(waiting, /aborted/)
  assert.equal(resetCount, 0)
  assert.deepEqual(initialized, [[canonicalSandboxPath(first)]])
  assert.deepEqual([...service.workspaces], [canonicalSandboxPath(first)])
  await running
  await service.dispose()
})

test('workspace reset failures fail closed and keep the previous grant registered', async (t) => {
  const first = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-reset-a-'))
  const second = await mkdtemp(join(tmpdir(), 'pisper-sandbox-win-reset-b-'))
  t.after(() => Promise.all([
    rm(first, { recursive: true, force: true }),
    rm(second, { recursive: true, force: true }),
  ]))
  let enabled = false
  let wrapped = 0
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => enabled,
    initialize: async () => { enabled = true },
    wrapWithSandboxArgv: async () => {
      wrapped += 1
      return { argv: [process.execPath, '-e', ''], env: {} }
    },
    cleanupAfterCommand: () => {},
    reset: async () => { throw new Error('reset failed') },
  }
  const service = new LocalSandboxService({ dataDir: join(tmpdir(), 'pisper-sandbox-reset-data'), homeDir: TEST_WINDOWS_HOME, platform: 'win32', manager, windowsShell: TEST_WINDOWS_SHELL })
  const operations = service.createBashOperations()

  await operations.exec('first', first, { onData: () => {}, timeout: 5 })
  await assert.rejects(
    operations.exec('second', second, { onData: () => {}, timeout: 5 }),
    /reset failed/,
  )
  assert.equal(wrapped, 1)
  assert.deepEqual([...service.workspaces], [canonicalSandboxPath(first)])
  assert.equal(service.lastError?.message, 'reset failed')
  await service.dispose()
})

test('disposing the sandbox terminates active process trees and rejects future commands', async (t) => {
  const workspace = await mkdtemp(join(tmpdir(), 'pisper-sandbox-dispose-'))
  t.after(() => rm(workspace, { recursive: true, force: true }))
  let enabled = false
  let resetCount = 0
  const manager = {
    isSupportedPlatform: () => true,
    isSandboxingEnabled: () => enabled,
    initialize: async () => { enabled = true },
    // Keep the command payload offline of Windows Git-Bash wrapping so this test only
    // exercises process-tree disposal and reject-after-dispose behavior.
    wrapWithSandboxArgv: async () => ({ argv: [process.execPath, '-e', 'setInterval(() => {}, 1000)'], env: {} }),
    cleanupAfterCommand: () => {},
    reset: async () => { enabled = false; resetCount += 1 },
  }
  const service = new LocalSandboxService({
    dataDir: join(tmpdir(), 'pisper-sandbox-dispose-data'),
    platform: 'linux',
    manager,
    disposeGraceMs: 1_000,
  })
  const operations = service.createBashOperations()
  const running = operations.exec('long-running', workspace, { onData: () => {} })
  for (let attempt = 0; attempt < 80 && service.activeChildren.size < 1; attempt += 1) await delay(10)
  assert.equal(service.activeChildren.size, 1)

  const startedAt = Date.now()
  await service.dispose()
  await running.catch(() => {})

  assert.ok(Date.now() - startedAt < 1_500)
  assert.equal(service.activeCommands, 0)
  assert.equal(service.activeChildren.size, 0)
  assert.equal(resetCount, 1)
  await assert.rejects(
    operations.exec('after-dispose', workspace, { onData: () => {}, timeout: 1 }),
    /正在关闭/,
  )
})

test('bash routing uses the sandbox unless full access or one-shot escalation is explicit', () => {
  assert.equal(shouldRunBashOutsideSandbox('workspace', {}), false)
  assert.equal(shouldRunBashOutsideSandbox('workspace', { sandbox_permissions: 'workspace' }), false)
  assert.equal(shouldRunBashOutsideSandbox('workspace', { sandbox_permissions: 'require_escalated' }), true)
  assert.equal(shouldRunBashOutsideSandbox('full-access', {}), true)
})
