import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SettingsManager } from '@earendil-works/pi-coding-agent'
import { SkillsService } from '../services/skills-service.mjs'

async function writeSkill(directory, name, description, extra = '') {
  await mkdir(directory, { recursive: true })
  await writeFile(join(directory, 'SKILL.md'), `---\nname: ${name}\ndescription: ${description}\n${extra}---\n\n# ${name}\n\nFollow these instructions.\n`, 'utf8')
}

test('skills service discovers Pi skills and applies persistent enable/invocation overrides', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skills-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(cwd, { recursive: true })
  await writeSkill(join(agentDir, 'skills', 'docs-search'), 'docs-search', 'Search official product documentation.', 'allowed-tools: read grep\n')

  const settingsManager = SettingsManager.inMemory({ enableSkillCommands: true })
  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => settingsManager,
  })
  await service.init()

  const initial = await service.dashboard()
  const skill = initial.skills.find((item) => item.name === 'docs-search')
  assert.ok(skill)
  assert.equal(skill.enabled, true)
  assert.equal(skill.modelInvocationEnabled, true)
  assert.deepEqual(skill.allowedTools, ['read', 'grep'])
  assert.equal(skill.command, '/skill:docs-search')
  assert.equal(skill.removable, false)
  await assert.rejects(service.remove(skill.id), /只能卸载由 Pisper 安装的技能/)

  const disabled = await service.update(skill.id, { enabled: false })
  assert.equal(disabled.enabled, false)
  const filteredLoader = await service.createResourceLoader(cwd)
  assert.equal(filteredLoader.getSkills().skills.some((item) => item.name === 'docs-search'), false)

  await service.update(skill.id, { enabled: true, modelInvocationEnabled: false })
  const manualLoader = await service.createResourceLoader(cwd)
  const manualSkill = manualLoader.getSkills().skills.find((item) => item.name === 'docs-search')
  assert.equal(manualSkill.disableModelInvocation, true)

  const restored = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => settingsManager,
  })
  await restored.init()
  const restoredSkill = (await restored.dashboard()).skills.find((item) => item.name === 'docs-search')
  assert.equal(restoredSkill.enabled, true)
  assert.equal(restoredSkill.modelInvocationEnabled, false)
})

test('skills service installs Pi package skill resources through DefaultPackageManager-compatible resolution', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skill-package-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  const packageSkill = join(directory, 'package', 'skills', 'package-helper')
  await mkdir(cwd, { recursive: true })
  await writeSkill(packageSkill, 'package-helper', 'Help with package workflows.')
  const calls = []
  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => SettingsManager.inMemory(),
    createPackageManager: () => ({
      async resolveExtensionSources(sources, options) {
        calls.push({ sources, options })
        return { extensions: [], prompts: [], themes: [], skills: [{ path: join(packageSkill, 'SKILL.md'), enabled: true }] }
      },
      listConfiguredPackages() {
        return [{ source: 'npm:fixture-skills', scope: 'user', filtered: false, installedPath: packageSkill }]
      },
    }),
  })
  await service.init()

  const installed = await service.install({ source: 'npm:fixture-skills' })
  assert.equal(installed.installed[0].name, 'package-helper')
  assert.deepEqual(calls[0], { sources: ['npm:fixture-skills'], options: { temporary: true } })
  assert.equal(installed.packages[0].source, 'npm:fixture-skills')
  assert.equal(installed.packages[0].installed, true)
})

test('skills dashboard uses single-flight caching and skills-only discovery', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skills-cache-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(cwd, { recursive: true })
  await writeSkill(join(agentDir, 'skills', 'cache-skill'), 'cache-skill', 'Validate dashboard caching.')

  let resolveCalls = 0
  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => SettingsManager.inMemory({ enableSkillCommands: true }),
    createPackageManager: () => ({
      async resolve() {
        resolveCalls += 1
        return {
          extensions: [],
          prompts: [],
          themes: [],
          skills: [{
            path: join(agentDir, 'skills', 'cache-skill'),
            enabled: true,
            metadata: { source: 'auto', scope: 'user', origin: 'top-level' },
          }],
        }
      },
      listConfiguredPackages() {
        return []
      },
    }),
  })
  await service.init()

  const [first, second] = await Promise.all([service.dashboard(), service.dashboard()])
  assert.equal(first.skills[0].name, 'cache-skill')
  assert.equal(second.skills[0].name, 'cache-skill')
  assert.equal(resolveCalls, 1)

  const cached = await service.dashboard()
  assert.equal(cached.skills[0].name, 'cache-skill')
  assert.equal(resolveCalls, 1)

  service.invalidateDashboardCache()
  const forced = await service.dashboard({ force: true })
  assert.equal(forced.skills[0].name, 'cache-skill')
  assert.equal(resolveCalls, 2)
})

test('skills service installs only skill resources from a local source and can remove Pisper-managed skills', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skill-install-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  const source = join(directory, 'external', 'release-notes')
  const emptySource = join(directory, 'external', 'empty')
  await mkdir(cwd, { recursive: true })
  await mkdir(emptySource, { recursive: true })
  await writeSkill(source, 'release-notes', 'Generate release notes from commits.')
  await writeFile(join(source, 'template.md'), '# Release template\n', 'utf8')

  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => SettingsManager.inMemory(),
  })
  await service.init()

  await assert.rejects(service.install({ source: emptySource }), /没有发现符合 Agent Skills 标准的技能/)
  const installed = await service.install({ source })
  assert.equal(installed.installed.length, 1)
  assert.equal(installed.installed[0].name, 'release-notes')
  assert.equal(installed.installed[0].removable, true)

  await assert.rejects(service.install({ source }), /已存在|已安装/)
  assert.equal(await service.remove(installed.installed[0].id), true)
  assert.equal((await service.dashboard()).skills.some((item) => item.name === 'release-notes'), false)
})
