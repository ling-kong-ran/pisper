import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

import { requiredRuntimeFeature } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { resolveRuntimeCapabilities } from '../runtime-capabilities.mjs'

const unavailableModules = {
  childProcess: false,
  workerThreads: false,
  sqlite: false,
  wasm: false,
}

test('mobile embedded profile derives degradation from actual host modules', async () => {
  const capabilities = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'mobile-embedded' },
    moduleSupport: unavailableModules,
  })

  assert.equal(capabilities.profile, 'mobile-embedded')
  assert.equal(capabilities.engine, 'node')
  assert.equal(capabilities.features.chat, true)
  assert.equal(capabilities.features.sessions, true)
  assert.equal(capabilities.features.providers, true)
  assert.equal(capabilities.features.filesystem, true)
  assert.equal(capabilities.features.shell, false)
  assert.equal(capabilities.features.vcs, false)
  assert.equal(capabilities.features.memory, false)
  assert.equal(capabilities.features.plugins, false)
  assert.equal(capabilities.features.workflows, false)
  assert.deepEqual(capabilities.tools, [
    'edit',
    'generate_visual',
    'ls',
    'mobile_device',
    'read',
    'skill_create',
    'web_search',
    'write',
  ])
})

test('iOS embedded profile disables process execution even when Node exposes child_process', async () => {
  const capabilities = await resolveRuntimeCapabilities({
    environment: {
      PISPER_RUNTIME_PROFILE: 'mobile-embedded',
      PISPER_RUNTIME_PLATFORM: 'ios',
    },
    moduleSupport: {
      childProcess: true,
      workerThreads: true,
      sqlite: true,
      wasm: true,
    },
  })

  assert.equal(capabilities.features.processes, false)
  assert.equal(capabilities.features.shell, false)
  assert.equal(capabilities.features.vcs, false)
  for (const unavailable of ['bash', 'grep', 'find']) {
    assert.equal(capabilities.tools.includes(unavailable), false, unavailable)
  }
})

test('store profile disables dynamic execution even when Node exposes the modules', async () => {
  const capabilities = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'mobile-store' },
    moduleSupport: {
      childProcess: true,
      workerThreads: true,
      sqlite: true,
      wasm: true,
    },
  })

  assert.equal(capabilities.profile, 'mobile-store')
  assert.equal(capabilities.features.processes, false)
  assert.equal(capabilities.features.shell, false)
  assert.equal(capabilities.features.vcs, false)
  assert.equal(capabilities.features.workers, false)
  assert.equal(capabilities.features.plugins, false)
  assert.equal(capabilities.features.mcp, false)
  for (const unavailable of ['bash', 'grep', 'find', 'plugin_create', 'mcp_list', 'mcp_manage']) {
    assert.equal(capabilities.tools.includes(unavailable), false, unavailable)
  }
})

test('unknown profile cannot claim unavailable capabilities', async () => {
  const capabilities = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'invented-mobile-runtime' },
    moduleSupport: unavailableModules,
  })

  assert.equal(capabilities.profile, 'desktop')
  assert.equal(capabilities.features.processes, false)
  assert.equal(capabilities.features.memory, false)
  assert.equal(capabilities.features.workers, false)
})

test('root mobile profile keeps Node services without claiming desktop-only bridges', async () => {
  const capabilities = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'mobile-root' },
    moduleSupport: {
      childProcess: true,
      workerThreads: true,
      sqlite: true,
      wasm: true,
    },
  })

  assert.equal(capabilities.features.shell, true)
  assert.equal(capabilities.features.vcs, true)
  assert.equal(capabilities.features.memory, true)
  assert.equal(capabilities.features.workflows, true)
  assert.equal(capabilities.features.terminal, false)
  assert.equal(capabilities.features.browserAutomation, false)
  assert.equal(capabilities.features.remoteAccess, false)
  assert.equal(capabilities.features.desktopPet, false)
})

test('embedded profile initializes the shared Agent runtime with only supported tools', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-runtime-capabilities-'))
  const capabilities = await resolveRuntimeCapabilities({
    environment: { PISPER_RUNTIME_PROFILE: 'mobile-embedded' },
    moduleSupport: unavailableModules,
  })
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory, capabilities })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })

  await runtime.init()
  const created = await runtime.createSession('Mobile session')
  const value = await runtime.getOrCreateSession(created.id)
  const tools = value.session
    .getAllTools()
    .map((tool) => tool.name)
    .sort()

  assert.equal(value.session.sessionId, created.id)
  assert.ok(tools.includes('read'))
  assert.ok(tools.includes('edit'))
  assert.ok(tools.includes('web_search'))
  assert.ok(tools.includes('mobile_device'))
  for (const unavailable of [
    'bash',
    'grep',
    'find',
    'memory_search',
    'mcp_list',
    'plugin_create',
    'spawn_agent',
    'get_plan',
  ]) {
    assert.equal(tools.includes(unavailable), false, unavailable)
  }

  const catalog = await runtime.getPlugins(created.id)
  const catalogTools = catalog.tools.map((tool) => tool.id)
  assert.ok(catalogTools.includes('read'))
  assert.ok(catalogTools.includes('web_search'))
  assert.equal(catalogTools.includes('bash'), false)
  assert.equal(catalogTools.includes('memory_search'), false)
  assert.equal(catalogTools.includes('mcp_list'), false)
  assert.equal(
    catalog.plugins.some((plugin) => !plugin.builtIn),
    false,
  )
})

test('capability-gated API groups map to their owning runtime feature', () => {
  assert.equal(requiredRuntimeFeature('/api/memory'), 'memory')
  assert.equal(requiredRuntimeFeature('/api/plugins', 'GET'), '')
  assert.equal(requiredRuntimeFeature('/api/plugins', 'PUT'), '')
  assert.equal(requiredRuntimeFeature('/api/plugins/inspect', 'POST'), 'plugins')
  assert.equal(requiredRuntimeFeature('/api/plugins/web-search/test', 'POST'), 'webSearch')
  assert.equal(requiredRuntimeFeature('/api/sessions/one/vcs/changes'), 'vcs')
  assert.equal(requiredRuntimeFeature('/api/sessions/one/goal'), 'goals')
  assert.equal(requiredRuntimeFeature('/api/workflows/one/run'), 'workflows')
  assert.equal(requiredRuntimeFeature('/api/providers'), '')
  assert.equal(requiredRuntimeFeature('/api/sessions'), '')
})

test('Pi compatibility patch removes eager optional builtin imports from headless paths', async () => {
  const repositoryRoot = fileURLToPath(new URL('../../', import.meta.url))
  const piRoot = join(repositoryRoot, 'node_modules', '@earendil-works', 'pi-coding-agent', 'dist')
  const checks = [
    ['utils/child-process.js', /from ["']node:child_process["']/],
    ['core/resolve-config-value.js', /from ["'](?:node:)?child_process["']/],
    ['core/exec.js', /from ["']node:child_process["']/],
    ['core/tools/grep.js', /from ["'](?:node:)?child_process["']/],
    ['core/tools/find.js', /from ["'](?:node:)?child_process["']/],
    ['utils/image-resize.js', /from ["']node:worker_threads["']/],
  ]
  for (const [file, eagerImport] of checks) {
    assert.doesNotMatch(await readFile(join(piRoot, file), 'utf8'), eagerImport, file)
  }
})
