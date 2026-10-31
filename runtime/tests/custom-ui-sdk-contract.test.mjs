// 内置技能 pisper-component 与 SDK 契约：.d.ts 的 Ctx / UiKit 成员必须等于 shared 常量；
// 技能写盘后能被 Pi 载入、不进模型自动调用列表（formatSkillsForPrompt 不含它），且能经 /skill: 显式展开。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SettingsManager, formatSkillsForPrompt, loadSkills } from '@earendil-works/pi-coding-agent'
import { CTX_V1_KEYS, UI_KIT_V1_KEYS } from '../../shared/custom-ui-manifest.mjs'
import {
  BUILTIN_SKILLS,
  PISPER_COMPONENT_DTS,
  PISPER_COMPONENT_SKILL_MD,
} from '../services/custom-ui-sdk-docs.mjs'
import { findBareImports } from '../services/custom-ui-service.mjs'
import { buildComponentTemplate, COMPONENT_TEMPLATES } from '../services/custom-ui-templates.mjs'
import { SkillsService } from '../services/skills-service.mjs'

/** 取 .d.ts 里某个 interface 的直接成员名（两个空格缩进的行）。 */
function interfaceMembers(source, name) {
  const match = new RegExp(`export interface ${name} \\{\\n([\\s\\S]*?)\\n\\}`).exec(source)
  assert.ok(match, `interface ${name} missing`)
  return match[1]
    .split('\n')
    .map((line) => /^ {2}([A-Za-z_$][\w$]*)\??[:(<]/.exec(line)?.[1])
    .filter(Boolean)
}

test('.d.ts 的 Ctx 成员等于 CTX_V1_KEYS，UiKit 成员等于 UI_KIT_V1_KEYS', () => {
  assert.deepEqual(interfaceMembers(PISPER_COMPONENT_DTS, 'Ctx'), [...CTX_V1_KEYS])
  assert.deepEqual(interfaceMembers(PISPER_COMPONENT_DTS, 'UiKit'), [...UI_KIT_V1_KEYS])
  // 技能正文内联同一份 .d.ts，ctx 表格逐项列出全部成员。
  assert.ok(PISPER_COMPONENT_SKILL_MD.includes(PISPER_COMPONENT_DTS))
  for (const key of CTX_V1_KEYS) assert.ok(PISPER_COMPONENT_SKILL_MD.includes(`\`${key}`), key)
  // 只用 v2 放置名，不出现旧名。
  assert.doesNotMatch(PISPER_COMPONENT_SKILL_MD, /board-card|more-page|1x1|2x1/)
  for (const surface of ['overview-card', 'session-panel', 'feature-page', 'floating'])
    assert.match(PISPER_COMPONENT_SKILL_MD, new RegExp(surface))
  // 模板字符串的转义已还原：示例代码里是真正的反引号与 ${}。
  assert.doesNotMatch(PISPER_COMPONENT_SKILL_MD, /\\[`$]/)
  assert.match(PISPER_COMPONENT_SKILL_MD, /html`<\$\{ui\.Skeleton\}/)
})

test('技能里的示例 component.js 没有裸导入（与写入校验同一条规则）', () => {
  const example = /## 4\. component\.js 契约\n\n~~~js\n([\s\S]*?)~~~/.exec(
    PISPER_COMPONENT_SKILL_MD,
  )
  assert.ok(example)
  assert.deepEqual(findBareImports(example[1]), [])
})

test('技能、.d.ts 与内置模板只引用运行时真实存在的 CSS 变量（不写 Tailwind 的 --color-* 编译期别名）', async () => {
  // src/index.css 的 @theme inline 只在编译期展开，--color-* 不会出现在运行时 CSS 里；组件写了它只会退回继承色。
  // 运行时可用的是 @theme 之前 :root / 主题块里声明的变量。
  const css = await readFile(new URL('../../src/index.css', import.meta.url), 'utf8')
  const themeStart = css.indexOf('@theme inline {')
  assert.ok(themeStart > 0)
  const declared = new Set(
    [...css.slice(0, themeStart).matchAll(/(--[a-zA-Z0-9-]+)\s*:/g)].map((match) => match[1]),
  )
  const sources = [
    PISPER_COMPONENT_SKILL_MD,
    PISPER_COMPONENT_DTS,
    ...COMPONENT_TEMPLATES.flatMap((template) =>
      buildComponentTemplate(template).files.map((file) => file.content),
    ),
  ]
  const referenced = new Set()
  for (const source of sources)
    for (const match of source.matchAll(/(?:var\(|token\(')(--[a-zA-Z0-9-]+)[)']/g))
      referenced.add(match[1])
  assert.ok(referenced.has('--text-muted'))
  for (const name of referenced) assert.ok(declared.has(name), `${name} 在运行时 CSS 里不存在`)
})

async function skillsService(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-builtin-skills-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const agentDir = join(directory, 'agent')
  const cwd = join(directory, 'workspace')
  const settingsManager = SettingsManager.inMemory({ enableSkillCommands: true })
  const create = () =>
    new SkillsService({
      path: join(agentDir, 'pisper-skills.json'),
      agentDir,
      cwd,
      getSettingsManager: () => settingsManager,
      builtinSkills: BUILTIN_SKILLS,
      builtinSkillsDir: join(agentDir, 'builtin-skills'),
    })
  return { agentDir, cwd, create }
}

test('内置技能写盘后可被 loadSkills 载入，formatSkillsForPrompt 不包含它', async (t) => {
  const { agentDir, create } = await skillsService(t)
  await create().init()
  const skillFile = join(agentDir, 'builtin-skills', 'pisper-component', 'SKILL.md')
  assert.equal(await readFile(skillFile, 'utf8'), PISPER_COMPONENT_SKILL_MD)
  assert.equal(
    await readFile(
      join(agentDir, 'builtin-skills', 'pisper-component', 'pisper-component.d.ts'),
      'utf8',
    ),
    PISPER_COMPONENT_DTS,
  )
  const loaded = loadSkills({
    cwd: agentDir,
    agentDir,
    skillPaths: [join(agentDir, 'builtin-skills')],
    includeDefaults: false,
  })
  const skill = loaded.skills.find((item) => item.name === 'pisper-component')
  assert.ok(skill)
  assert.equal(skill.disableModelInvocation, true)
  assert.equal(formatSkillsForPrompt([skill]), '')
  assert.doesNotMatch(formatSkillsForPrompt(loaded.skills), /pisper-component/)
})

test('内置技能出现在技能页的全局技能里（来源 builtin、不可卸载、可停用），会话里可用 /skill: 加载', async (t) => {
  const { cwd, create } = await skillsService(t)
  const service = create()
  await service.init()
  const dashboard = await service.dashboard({ force: true })
  const skill = dashboard.skills.find((item) => item.name === 'pisper-component')
  assert.ok(skill)
  assert.equal(skill.sourceInfo.origin, 'builtin')
  assert.equal(skill.sourceInfo.scope, 'user')
  assert.equal(skill.removable, false)
  assert.equal(skill.modelInvocationEnabled, false)
  assert.equal(skill.command, '/skill:pisper-component')
  const loader = await service.createResourceLoader(cwd)
  assert.ok(loader.getSkills().skills.some((item) => item.name === 'pisper-component'))
  await service.update(skill.id, { enabled: false })
  const disabled = await service.createResourceLoader(cwd)
  assert.equal(
    disabled.getSkills().skills.some((item) => item.name === 'pisper-component'),
    false,
  )
})

test('内置技能按内容写入：内容没变不改文件，被改坏后下次启动还原', async (t) => {
  const { agentDir, create } = await skillsService(t)
  await create().init()
  const skillFile = join(agentDir, 'builtin-skills', 'pisper-component', 'SKILL.md')
  const before = (await stat(skillFile)).mtimeMs
  await new Promise((resolveDelay) => setTimeout(resolveDelay, 20))
  await create().init()
  assert.equal((await stat(skillFile)).mtimeMs, before)
  await writeFile(skillFile, 'tampered')
  await create().init()
  assert.equal(await readFile(skillFile, 'utf8'), PISPER_COMPONENT_SKILL_MD)
})
