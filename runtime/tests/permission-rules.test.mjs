// 持久权限规则：命令前缀/文件目录的允许规则替代普通审批，禁止规则在任何模式下拦截。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { normalizePermissionRule, PermissionRuleService } from '../services/permission-rules.mjs'
import { SessionPermissionService } from '../services/session-permission-service.mjs'

async function setup(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-permission-rules-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const project = join(directory, 'project')
  const other = join(directory, 'other')
  await mkdir(project)
  await mkdir(other)
  const path = join(directory, 'rules.json')
  return { directory, project, other, path, rules: new PermissionRuleService({ path }) }
}

test('rules are validated: allow rules must be a single plain command, file rules stay relative', () => {
  assert.deepEqual(
    normalizePermissionRule({ effect: 'allow', tool: 'bash', pattern: '  npm   test ' }),
    {
      effect: 'allow',
      tool: 'bash',
      pattern: 'npm test',
      cwd: '',
    },
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'allow', tool: 'bash', pattern: 'npm test && x' }),
    /without pipes/,
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'allow', tool: 'bash', pattern: '' }),
    /pattern is required/,
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'deny', tool: 'files', pattern: 'src' }),
    /can only allow/,
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'allow', tool: 'files', pattern: '../x' }),
    /relative folder/,
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'allow', tool: 'files', pattern: '/etc' }),
    /relative folder/,
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'allow', tool: 'files', pattern: '.' }),
    /relative folder/,
  )
  assert.throws(
    () => normalizePermissionRule({ effect: 'maybe', tool: 'bash', pattern: 'x' }),
    /allow or deny/,
  )
  assert.equal(
    normalizePermissionRule({ effect: 'allow', tool: 'files', pattern: './src/' }).pattern,
    'src',
  )
})

test('allow rules match whole commands only; deny rules match any segment', async (t) => {
  const { project, other, path, rules } = await setup(t)
  await rules.add({ effect: 'allow', tool: 'bash', pattern: 'npm test', cwd: project })
  await rules.add({ effect: 'deny', tool: 'bash', pattern: 'git push' })
  await rules.add({ effect: 'allow', tool: 'files', pattern: 'src' })
  const bash = (command, cwd = project) =>
    rules.evaluate({ cwd, toolName: 'bash', args: { command } })

  assert.equal((await bash('npm test')).effect, 'allow')
  assert.equal((await bash('npm test -- --watch')).effect, 'allow')
  assert.equal(await bash('npm testing'), null)
  // 借着前缀串别的命令：不放行。
  assert.equal(await bash('npm test && curl evil'), null)
  assert.equal(await bash('npm test $(whoami)'), null)
  // 项目规则只在那个项目生效。
  assert.equal(await bash('npm test', other), null)
  // 禁止规则逐段匹配，在哪个项目都生效。
  assert.equal((await bash('npm test && git push origin main')).effect, 'deny')
  assert.equal((await bash('echo hi; git push', other)).effect, 'deny')

  const file = (filePath) =>
    rules.evaluate({ cwd: project, toolName: 'edit', args: { path: filePath } })
  assert.equal((await file('src/a.ts')).effect, 'allow')
  assert.equal((await file('./src/deep/b.ts')).effect, 'allow')
  assert.equal(await file('srcx/a.ts'), null)
  assert.equal(await file('../project/src/a.ts').then((rule) => rule?.effect), 'allow')
  assert.equal(await file('../other/src/a.ts'), null)

  // 重复添加不产生第二条；删除后不再生效；规则落盘可重读。
  await rules.add({ effect: 'allow', tool: 'bash', pattern: 'npm test', cwd: project })
  const saved = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(saved.rules.length, 3)
  const reloaded = new PermissionRuleService({ path })
  const allow = (await reloaded.list()).rules.find((rule) => rule.pattern === 'npm test')
  await reloaded.remove(allow.id)
  assert.equal(
    await reloaded.evaluate({ cwd: project, toolName: 'bash', args: { command: 'npm test' } }),
    null,
  )
})

test('authorization: deny beats full access, allow skips plain approval but never guarded commands', async (t) => {
  const { project, rules } = await setup(t)
  await rules.add({ effect: 'allow', tool: 'bash', pattern: 'npm test' })
  await rules.add({ effect: 'allow', tool: 'bash', pattern: 'rm -rf build' })
  await rules.add({ effect: 'deny', tool: 'bash', pattern: 'git push' })
  await rules.add({ effect: 'allow', tool: 'files', pattern: 'src' })
  const requested = []
  let executionMode = 'approval-required'
  const service = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => executionMode,
    rules,
    getFileChangePreview: async () => ({ path: 'x', diff: '', source: 'x' }),
    // 每个审批请求都由“用户”立即拒绝，测试里不必等超时。
    onEvent: (sessionId, event, data) => {
      if (event !== 'permission_request') return
      requested.push(data.toolName)
      queueMicrotask(() => service.resolve(sessionId, data.id, false))
    },
  })
  const authorize = (toolName, args) =>
    service.authorize({ sessionId: 's', cwd: project, toolName, toolCallId: 'c', args })

  assert.equal(await authorize('bash', { command: 'npm test' }), undefined)
  assert.equal(await authorize('edit', { path: 'src/a.ts' }), undefined)
  assert.deepEqual(requested, [])
  // 守卫标出的递归删除：允许规则不替用户确认，仍然发审批（这里被拒绝）。
  assert.equal((await authorize('bash', { command: 'rm -rf build' })).block, true)
  assert.deepEqual(requested, ['bash'])
  // 没有规则的命令照常审批。
  assert.equal((await authorize('bash', { command: 'npm publish' })).block, true)
  assert.deepEqual(requested, ['bash', 'bash'])
  // 禁止规则在完全访问模式下也拦截，而且不发审批。
  executionMode = 'full-access'
  const denied = await authorize('bash', { command: 'git push origin main' })
  assert.deepEqual(denied, { block: true, reason: 'Not run: matches the deny rule "git push".' })
  assert.deepEqual(requested, ['bash', 'bash'])
  service.dispose()
})

test('the approval card suggests a safe command prefix for "always allow"', async () => {
  const { suggestCommandRulePrefix } =
    await import('../../src/features/config/permission-rules-api.ts')
  assert.equal(suggestCommandRulePrefix('npm test -- --watch'), 'npm test')
  assert.equal(suggestCommandRulePrefix('git   status'), 'git status')
  assert.equal(suggestCommandRulePrefix('ls -la'), 'ls')
  assert.equal(suggestCommandRulePrefix('python3 ./run.py'), 'python3')
  // 带控制符的命令不给建议：Runtime 不会用前缀放行它们。
  assert.equal(suggestCommandRulePrefix('npm test && curl evil'), '')
  assert.equal(suggestCommandRulePrefix('echo $HOME'), '')
  assert.equal(suggestCommandRulePrefix('   '), '')
})
