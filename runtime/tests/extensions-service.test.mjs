import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SettingsManager } from '@earendil-works/pi-coding-agent'
import { ExtensionsService } from '../services/extensions-service.mjs'
import { createDefaultResourceLoader } from '../runtime/pi-coding-agent.mjs'

async function writeExtension(path, { tool = '', command = '', event = '' } = {}) {
  await mkdir(join(path, '..'), { recursive: true })
  await writeFile(
    path,
    `import { Type } from 'typebox'
export default function (pi) {
  ${tool ? `pi.registerTool({ name: '${tool}', label: '${tool}', description: 'Fixture tool', parameters: Type.Object({}), async execute() { return { content: [{ type: 'text', text: 'ok' }], details: {} } } })` : ''}
  ${command ? `pi.registerCommand('${command}', { description: 'Fixture command', handler: async () => {} })` : ''}
  ${event ? `pi.on('${event}', async () => {})` : ''}
}
`,
    'utf8',
  )
}

function createService({ agentDir, cwd, settingsManager, createPackageManager }) {
  const createResourceLoader = async (targetCwd) => {
    const loader = await createDefaultResourceLoader({
      cwd: targetCwd,
      agentDir,
      settingsManager,
    })
    await loader.reload()
    return loader
  }
  return new ExtensionsService({
    agentDir,
    cwd,
    getSettingsManager: () => settingsManager,
    createResourceLoader,
    createPackageManager,
  })
}

test('extensions service projects loaded capabilities and Pi conflict diagnostics', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-extensions-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(cwd, { recursive: true })
  await writeExtension(join(agentDir, 'extensions', 'first.ts'), {
    tool: 'fixture_echo',
    command: 'fixture',
    event: 'session_start',
  })
  await writeExtension(join(agentDir, 'extensions', 'second.ts'), { tool: 'fixture_echo' })
  const settingsManager = SettingsManager.inMemory({}, { projectTrusted: true })
  const service = createService({ agentDir, cwd, settingsManager })

  const dashboard = await service.dashboard()
  const first = dashboard.extensions.find((item) => item.name === 'first')
  assert.ok(first)
  assert.equal(first.loaded, true)
  assert.deepEqual(first.capabilities.tools, ['fixture_echo'])
  assert.deepEqual(first.capabilities.commands, ['fixture'])
  assert.deepEqual(first.capabilities.events, ['session_start'])
  assert.equal(dashboard.counts.collisions, 1)
  assert.match(
    dashboard.diagnostics.find((item) => item.type === 'collision')?.message || '',
    /fixture_echo.*conflicts with/,
  )
})

test('extensions service persists top-level enable overrides using Pi config patterns', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-extension-toggle-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(cwd, { recursive: true })
  await writeExtension(join(agentDir, 'extensions', 'toggle.ts'), { tool: 'toggle_fixture' })
  const settingsManager = SettingsManager.inMemory({}, { projectTrusted: true })
  const service = createService({ agentDir, cwd, settingsManager })
  const extension = (await service.dashboard()).extensions.find((item) => item.name === 'toggle')
  assert.ok(extension)

  const disabled = await service.updateExtension(extension.id, false)
  assert.equal(disabled.extensions.find((item) => item.id === extension.id)?.enabled, false)
  assert.ok(
    settingsManager.getGlobalSettings().extensions.some((item) => item === '-extensions/toggle.ts'),
  )

  const enabled = await service.updateExtension(extension.id, true)
  assert.equal(enabled.extensions.find((item) => item.id === extension.id)?.enabled, true)
  assert.ok(
    settingsManager.getGlobalSettings().extensions.some((item) => item === '+extensions/toggle.ts'),
  )
})

test('extensions service gates project writes on workspace trust', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-project-extension-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await writeExtension(join(cwd, '.pi', 'extensions', 'project.ts'), {
    event: 'session_start',
  })
  const settingsManager = SettingsManager.inMemory({}, { projectTrusted: false })
  const service = createService({ agentDir, cwd, settingsManager })

  const restricted = await service.dashboard()
  assert.equal(restricted.trusted, false)
  assert.equal(
    restricted.extensions.some((item) => item.scope === 'project'),
    false,
  )
  await assert.rejects(
    service.install({ source: './local-extension', scope: 'project' }),
    /先信任当前工作区/,
  )

  settingsManager.setProjectTrusted(true)
  service.invalidate()
  const trusted = await service.dashboard({ force: true })
  assert.equal(
    trusted.extensions.some((item) => item.name === 'project'),
    true,
  )
})

test('extensions service redacts configured package sources and routes updates by stable id', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-extension-package-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const settingsManager = SettingsManager.inMemory({}, { projectTrusted: true })
  const source = 'git+ssh://user:password@example.com/extensions.git?token=secret&channel=next'
  const calls = []
  const packageManager = {
    async resolve() {
      return { extensions: [], errors: [] }
    },
    listConfiguredPackages() {
      return [{ source, scope: 'user', installed: true, filtered: false }]
    },
    async update(input) {
      calls.push(['update', input])
    },
    async removeAndPersist(input, options) {
      calls.push(['remove', input, options])
      return true
    },
  }
  const service = createService({
    agentDir: directory,
    cwd: directory,
    settingsManager,
    createPackageManager: () => packageManager,
  })

  const dashboard = await service.dashboard({ force: true })
  assert.equal(dashboard.packages.length, 1)
  assert.doesNotMatch(dashboard.packages[0].source, /user|password|secret/)
  assert.match(dashboard.packages[0].source, /channel=next/)
  const packageId = dashboard.packages[0].id

  await service.updatePackage(packageId)
  await service.removePackage(packageId)
  assert.deepEqual(calls, [
    ['update', source],
    ['remove', source, { local: false }],
  ])
  await assert.rejects(
    service.install({ source: 'https://user:password@example.com/private.git', scope: 'user' }),
    /不能包含内嵌凭据/,
  )
})

test('extensions service captures registration and runtime event diagnostics', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-extension-events-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const settingsManager = SettingsManager.inMemory({}, { projectTrusted: true })
  const service = createService({ agentDir: directory, cwd: directory, settingsManager })
  const runner = {
    getRegisteredCommands() {
      return []
    },
    getCommandDiagnostics() {
      return [{ type: 'collision', message: 'Command collision', path: '/fixture/command.ts' }]
    },
  }

  service.recordRuntime('session-1', directory, runner)
  for (let index = 0; index < 105; index += 1) {
    service.recordRuntimeError('session-1', directory, {
      extensionPath: '/fixture/event.ts',
      event: 'session_start',
      error: `startup failed ${index}`,
    })
  }

  const runtimeDiagnostics = service.runtimeDiagnosticsFor(directory)
  assert.equal(runtimeDiagnostics.length, 100)
  assert.deepEqual(
    runtimeDiagnostics.slice(-2).map((item) => [item.phase, item.type, item.message]),
    [
      ['event', 'error', 'startup failed 103'],
      ['event', 'error', 'startup failed 104'],
    ],
  )
})
