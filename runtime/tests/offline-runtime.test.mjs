import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SettingsManager, createDefaultPackageManager } from '../runtime/pi-coding-agent.mjs'
import { ProviderPreferences } from '../runtime/provider-preferences.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { SkillsService } from '../services/skills-service.mjs'

test('resource discovery skips missing packages without installing and retains local skills', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-offline-resources-'))
  const previousAgentDir = process.env.PI_CODING_AGENT_DIR
  t.after(async () => {
    if (previousAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR
    else process.env.PI_CODING_AGENT_DIR = previousAgentDir
    await rm(directory, { recursive: true, force: true })
  })
  const agentDir = join(directory, 'agent')
  const localPackage = join(directory, 'local-package')
  await mkdir(join(localPackage, 'skills', 'offline-helper'), { recursive: true })
  await writeFile(
    join(localPackage, 'package.json'),
    JSON.stringify({ name: 'offline-helper', pi: { skills: ['./skills'] } }),
  )
  await writeFile(
    join(localPackage, 'skills', 'offline-helper', 'SKILL.md'),
    '---\nname: offline-helper\ndescription: Local offline helper\n---\nRead local files.\n',
  )
  const missingNpm = 'npm:@pisper-offline-test/absent@1.0.0'
  const packages = [missingNpm, 'git:https://example.test/pisper/absent', localPackage]
  const settingsManager = SettingsManager.inMemory({ packages })
  const manager = await createDefaultPackageManager({ cwd: directory, agentDir, settingsManager })
  const installationRequested = new Error('explicit installation requested')
  const install = t.mock.method(Object.getPrototypeOf(manager), 'runCommand', async () => {
    throw installationRequested
  })
  for (const method of ['runCommandCapture', 'runCommandSync']) {
    t.mock.method(Object.getPrototypeOf(manager), method, () => {
      throw new Error('resource discovery must not run external commands')
    })
  }
  const configPath = join(directory, 'pisper.json')
  await writeFile(configPath, JSON.stringify({ computerUseEnabled: false }))
  const service = new SkillsService({
    path: join(agentDir, 'skills.json'),
    agentDir,
    cwd: directory,
    configPath,
    getSettingsManager: () => settingsManager,
  })
  await service.init()
  const loader = await service.createResourceLoader(directory)
  assert.ok(loader.getSkills().skills.some((skill) => skill.name === 'offline-helper'))
  assert.equal(install.mock.callCount(), 0)
  // 缺包仍保留在配置与管理界面，用户显式安装的入口没有被禁用。
  assert.deepEqual(settingsManager.getGlobalSettings().packages, packages)
  assert.equal(
    manager.listConfiguredPackages().find((item) => item.source === missingNpm).installedPath,
    undefined,
  )
  await assert.rejects(manager.install(missingNpm), (error) => error === installationRequested)
  assert.equal(install.mock.callCount(), 1)
})

test('selecting an internal model completes while public metadata is still pending', async () => {
  const lookup = Promise.withResolvers()
  const model = { id: 'internal-model', provider: 'intranet', contextWindow: 128_000 }
  let calls = 0
  const service = new ProviderPreferences({
    modelMetadata: {
      ensure: () => {
        calls += 1
        return lookup.promise
      },
    },
    getModelRuntime: () => ({ getModel: () => model }),
  })
  try {
    const result = await Promise.race([
      service.resolveSessionModel('intranet', 'internal-model', { requireEnabled: false }),
      new Promise((resolve) => setImmediate(() => resolve('blocked on public metadata'))),
    ])
    assert.equal(result, model)
    assert.equal(calls, 1)
  } finally {
    lookup.resolve(null)
  }
})

test(
  'a fresh runtime and its restart open an internal-model session with public fetch unavailable',
  { timeout: 15_000 },
  async (t) => {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-offline-startup-'))
    const previousAgentDir = process.env.PI_CODING_AGENT_DIR
    let runtime
    t.after(async () => {
      await runtime?.dispose()
      if (previousAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR
      else process.env.PI_CODING_AGENT_DIR = previousAgentDir
      await rm(directory, { recursive: true, force: true })
    })
    t.mock.method(globalThis, 'fetch', () =>
      Promise.reject(new Error('public network unavailable')),
    )
    await writeFile(
      join(directory, 'settings.json'),
      JSON.stringify({ defaultProvider: 'intranet', defaultModel: 'private-model' }),
    )
    await writeFile(join(directory, 'pisper.json'), JSON.stringify({ computerUseEnabled: false }))
    await writeFile(
      join(directory, 'auth.json'),
      JSON.stringify({ intranet: { type: 'api_key', key: 'offline-test-key' } }),
    )
    await writeFile(
      join(directory, 'models.json'),
      JSON.stringify({
        providers: {
          intranet: {
            api: 'openai-completions',
            baseUrl: 'http://127.0.0.1:1/v1',
            models: [{ id: 'private-model', contextWindow: 128_000, maxTokens: 4096 }],
          },
        },
      }),
    )
    let id
    for (let restart = 0; restart < 2; restart += 1) {
      runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
      let metadataSignal
      runtime.modelMetadata.fetchImpl = (_url, { signal }) => {
        metadataSignal = signal
        return new Promise((_resolve, reject) => {
          signal.addEventListener('abort', () => reject(signal.reason), { once: true })
        })
      }
      await runtime.init()
      id ||= (await runtime.createSession('Offline session', directory)).id
      const active = await runtime.getOrCreateSession(id)
      assert.equal(active.session.model.id, 'private-model')
      assert.equal(active.session.model.contextWindow, 128_000)
      assert.equal(
        metadataSignal.aborted,
        false,
        'opening must finish before public lookup times out',
      )
      await runtime.dispose()
      assert.equal(metadataSignal.aborted, true)
      runtime = null
    }
  },
)
