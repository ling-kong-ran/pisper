import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SettingsManager } from '@earendil-works/pi-coding-agent'
import { SkillsService } from '../services/skills-service.mjs'

async function writeSkill(directory, name, description, extra = '') {
  await mkdir(directory, { recursive: true })
  await writeFile(
    join(directory, 'SKILL.md'),
    `---\nname: ${name}\ndescription: ${description}\n${extra}---\n\n# ${name}\n\nFollow these instructions.\n`,
    'utf8',
  )
}

test('skills service discovers Pi skills and applies persistent enable/invocation overrides', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skills-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(cwd, { recursive: true })
  await writeSkill(
    join(agentDir, 'skills', 'docs-search'),
    'docs-search',
    'Search official product documentation.',
    'allowed-tools: read grep\n',
  )

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
  assert.equal(
    filteredLoader.getSkills().skills.some((item) => item.name === 'docs-search'),
    false,
  )

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
  const restoredSkill = (await restored.dashboard()).skills.find(
    (item) => item.name === 'docs-search',
  )
  assert.equal(restoredSkill.enabled, true)
  assert.equal(restoredSkill.modelInvocationEnabled, false)
})

test('skills service trust gates project skills while Pi Extensions stay disabled', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-untrusted-skills-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await writeSkill(join(agentDir, 'skills', 'global-helper'), 'global-helper', 'Global helper.')
  await writeSkill(
    join(cwd, '.pisper', 'skills', 'project-helper'),
    'project-helper',
    'Project helper.',
  )
  await mkdir(join(cwd, '.pisper', 'prompts'), { recursive: true })
  await writeFile(
    join(cwd, '.pisper', 'prompts', 'project-review.md'),
    '---\ndescription: Review this project\nargument-hint: "<path>"\n---\nReview $1.\n',
    'utf8',
  )
  const extensionMarker = join(directory, 'extension-loaded')
  await mkdir(join(cwd, '.pi', 'extensions'), { recursive: true })
  await writeFile(
    join(cwd, '.pi', 'extensions', 'project-extension.ts'),
    `import { writeFileSync } from 'node:fs'\nwriteFileSync(${JSON.stringify(extensionMarker)}, 'loaded')\nexport default function () {}\n`,
    'utf8',
  )

  const settingsManager = SettingsManager.inMemory({}, { projectTrusted: false })
  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => settingsManager,
  })
  await service.init()

  const restricted = await service.discover(cwd)
  assert.deepEqual(
    restricted.skills.map((skill) => skill.name),
    ['global-helper'],
  )
  const restrictedLoader = await service.createResourceLoader(cwd)
  assert.deepEqual(
    restrictedLoader.getSkills().skills.map((skill) => skill.name),
    ['global-helper'],
  )
  assert.deepEqual(restrictedLoader.getPrompts().prompts, [])
  assert.equal(existsSync(extensionMarker), false)
  await assert.rejects(
    service.create({
      name: 'new-project-skill',
      description: 'Project only.',
      instructions: 'Run.',
    }),
    /先信任当前工作区/,
  )

  settingsManager.setProjectTrusted(true)
  const trusted = await service.discover(cwd)
  assert.deepEqual(trusted.skills.map((skill) => skill.name).sort(), [
    'global-helper',
    'project-helper',
  ])
  const trustedLoader = await service.createResourceLoader(cwd)
  assert.deepEqual(
    trustedLoader
      .getPrompts()
      .prompts.map((prompt) => [prompt.name, prompt.argumentHint, prompt.sourceInfo.scope]),
    [['project-review', '<path>', 'project']],
  )
  assert.equal(existsSync(extensionMarker), false)
})

test('skills service creates validated project and global skills without overwriting', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skill-create-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(cwd, { recursive: true })

  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => SettingsManager.inMemory({ enableSkillCommands: true }),
  })
  await service.init()

  const project = await service.create({
    name: 'release-helper',
    description: 'Creates release notes from commits. Use for product releases.',
    instructions: '# Release Helper\n\n1. Inspect commits.\n2. Write concise notes.',
  })
  assert.equal(project.scope, 'project')
  assert.equal(project.filePath, join(cwd, '.pisper', 'skills', 'release-helper', 'SKILL.md'))
  assert.equal(project.command, '/skill:release-helper')
  assert.match(
    await readFile(project.filePath, 'utf8'),
    /^---\nname: release-helper\ndescription: "Creates release notes from commits\. Use for product releases\."\n---/,
  )

  const global = await service.create({
    name: 'global-helper',
    description: 'Applies a reusable workflow. Use across projects.',
    instructions: '# Global Helper\n\nFollow the reusable workflow.',
    scope: 'global',
  })
  assert.equal(global.scope, 'global')
  assert.equal(global.filePath, join(agentDir, 'skills', 'global-helper', 'SKILL.md'))

  await assert.rejects(
    service.create({
      name: 'release-helper',
      description: 'Replacement description.',
      instructions: '# Replacement',
    }),
    /已存在.*不能覆盖/,
  )
  assert.match(await readFile(project.filePath, 'utf8'), /Inspect commits/)
})

test('skills service rejects invalid create input and occupied directories', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skill-create-invalid-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  await mkdir(join(cwd, '.pisper', 'skills', 'occupied'), { recursive: true })

  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => SettingsManager.inMemory(),
  })
  await service.init()

  await assert.rejects(
    service.create({ name: '../escape', description: 'Invalid.', instructions: '# Invalid' }),
    /技能名称必须/,
  )
  await assert.rejects(
    service.create({
      name: 'invalid-scope',
      description: 'Invalid.',
      instructions: '# Invalid',
      scope: 'shared',
    }),
    /作用域必须为 project 或 global/,
  )
  await assert.rejects(
    service.create({ name: 'empty-description', instructions: '# Invalid' }),
    /技能描述不能为空/,
  )
  await assert.rejects(
    service.create({ name: 'empty-instructions', description: 'Invalid.' }),
    /技能说明不能为空/,
  )
  await assert.rejects(
    service.create({ name: 'occupied', description: 'Occupied.', instructions: '# Occupied' }),
    /目录.*已存在.*不能覆盖/,
  )
})

test('skills service rejects project Skill directories linked outside the workspace', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skill-create-link-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  const outside = join(directory, 'outside')
  await mkdir(join(cwd, '.pisper'), { recursive: true })
  await mkdir(outside)
  await symlink(
    outside,
    join(cwd, '.pisper', 'skills'),
    process.platform === 'win32' ? 'junction' : 'dir',
  )

  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd,
    getSettingsManager: () => SettingsManager.inMemory(),
  })
  await service.init()

  await assert.rejects(
    service.create({
      name: 'escaped-helper',
      description: 'Must remain in the workspace.',
      instructions: '# Escaped Helper',
    }),
    /符号链接指向当前工作目录之外/,
  )
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
        return {
          extensions: [],
          prompts: [],
          themes: [],
          skills: [{ path: join(packageSkill, 'SKILL.md'), enabled: true }],
        }
      },
      listConfiguredPackages() {
        return [
          {
            source: 'npm:fixture-skills',
            scope: 'user',
            filtered: false,
            installedPath: packageSkill,
          },
        ]
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
  await writeSkill(
    join(agentDir, 'skills', 'cache-skill'),
    'cache-skill',
    'Validate dashboard caching.',
  )

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
          skills: [
            {
              path: join(agentDir, 'skills', 'cache-skill'),
              enabled: true,
              metadata: { source: 'auto', scope: 'user', origin: 'top-level' },
            },
          ],
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

  await assert.rejects(
    service.install({ source: emptySource }),
    /没有发现符合 Agent Skills 标准的技能/,
  )
  const installed = await service.install({ source })
  assert.equal(installed.installed.length, 1)
  assert.equal(installed.installed[0].name, 'release-notes')
  assert.equal(installed.installed[0].removable, true)

  await assert.rejects(service.install({ source }), /已存在|已安装/)
  assert.equal(await service.remove(installed.installed[0].id), true)
  assert.equal(
    (await service.dashboard()).skills.some((item) => item.name === 'release-notes'),
    false,
  )
})

test('skills dashboard separates global skills from the requested project workspace', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-skill-scopes-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const projectA = join(directory, 'project-a')
  const projectB = join(directory, 'project-b')
  await writeSkill(
    join(agentDir, 'skills', 'global-helper'),
    'global-helper',
    'Available in every project.',
  )
  await writeSkill(join(agentDir, 'skills', 'shared-helper'), 'shared-helper', 'Global version.')
  await writeSkill(
    join(projectA, '.pisper', 'skills', 'project-a-helper'),
    'project-a-helper',
    'Available only in project A.',
  )
  await writeSkill(
    join(projectA, '.pisper', 'skills', 'shared-helper'),
    'shared-helper',
    'Project version.',
  )
  await writeSkill(
    join(projectB, '.pisper', 'skills', 'project-b-helper'),
    'project-b-helper',
    'Available only in project B.',
  )
  await writeSkill(
    join(projectA, '.agents', 'skills', 'legacy-agents-helper'),
    'legacy-agents-helper',
    'Must not be loaded by Pisper.',
  )
  await writeSkill(
    join(projectA, '.pi', 'skills', 'legacy-pi-helper'),
    'legacy-pi-helper',
    'Must not be loaded by Pisper.',
  )

  const service = new SkillsService({
    path: join(agentDir, 'pisper-skills.json'),
    agentDir,
    cwd: projectA,
    getSettingsManager: (cwd) => SettingsManager.create(cwd, agentDir),
  })
  await service.init()

  const dashboardA = await service.dashboard({ cwd: projectA, force: true })
  assert.equal(dashboardA.cwd, projectA)
  assert.deepEqual(dashboardA.locations, {
    global: join(agentDir, 'skills'),
    project: join(projectA, '.pisper', 'skills'),
  })
  assert.deepEqual(
    new Set(dashboardA.skills.map((skill) => skill.name)),
    new Set(['global-helper', 'shared-helper', 'project-a-helper']),
  )
  assert.equal(
    dashboardA.skills.find((skill) => skill.name === 'global-helper')?.sourceInfo?.scope,
    'user',
  )
  assert.equal(
    dashboardA.skills.find((skill) => skill.name === 'project-a-helper')?.sourceInfo?.scope,
    'project',
  )
  assert.equal(
    dashboardA.skills.find((skill) => skill.name === 'shared-helper')?.description,
    'Project version.',
  )
  assert.equal(
    dashboardA.skills.some((skill) => skill.name === 'legacy-agents-helper'),
    false,
  )
  assert.equal(
    dashboardA.skills.some((skill) => skill.name === 'legacy-pi-helper'),
    false,
  )
  assert.equal(dashboardA.counts.global, 1)
  assert.equal(dashboardA.counts.project, 2)

  const dashboardB = await service.dashboard({ cwd: projectB, force: true })
  assert.deepEqual(
    new Set(dashboardB.skills.map((skill) => skill.name)),
    new Set(['global-helper', 'shared-helper', 'project-b-helper']),
  )
  assert.equal(
    dashboardB.skills.some((skill) => skill.name === 'project-a-helper'),
    false,
  )
  assert.equal(dashboardB.counts.global, 2)
  assert.equal(dashboardB.counts.project, 1)
})
