import assert from 'node:assert/strict'
import { execFileSync, spawnSync } from 'node:child_process'
import { chmod, cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import vm from 'node:vm'
import yaml from 'js-yaml'
import { fromMarkdown } from 'mdast-util-from-markdown'
import {
  assertReleaseNotes,
  formatReleaseNotes,
  releaseNotesDate,
} from '../../scripts/release-notes-format.mjs'

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../..')
const channels = [
  ['desktop', 'Desktop', 'v'],
  ['runtime', 'Runtime', 'runtime-v'],
  ['tui', 'TUI', 'tui-v'],
  ['app', 'App', 'app-v'],
]
const productPaths = ['src/example.ts', 'runtime/example.mjs', 'src-tui/src/main.rs']

async function fixture(t, { tags = true } = {}) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-release-notes-'))
  t.after(() => rm(root, { recursive: true, force: true }))
  const env = {
    ...process.env,
    GIT_CONFIG_NOSYSTEM: '1',
    GIT_CONFIG_GLOBAL: '/dev/null',
    GITHUB_REPOSITORY: 'example/pisper',
  }
  const git = (args) =>
    execFileSync('git', args, { cwd: root, env, encoding: 'utf8', stdio: 'pipe' }).trim()
  const put = async (path, value) => {
    await mkdir(dirname(join(root, path)), { recursive: true })
    await writeFile(join(root, path), value)
  }
  for (const name of [
    'prepare-release.mjs',
    'prepare-app-release.mjs',
    'release-changes.mjs',
    'release-components.mjs',
    'app-paths.mjs',
    'release-notes-format.mjs',
    'validate-release-notes.mjs',
  ]) {
    const source = join(repositoryRoot, 'scripts', name)
    if (!existsSync(source)) continue
    await mkdir(join(root, 'scripts'), { recursive: true })
    await cp(source, join(root, 'scripts', name))
  }
  await put('package.json', JSON.stringify({ type: 'module', version: '1.0.0' }))
  await put('src-tauri/desktop-package.json', '{"version":"1.0.0"}')
  await put('src-tauri/mobile-package.json', '{"version":"1.0.0"}')
  await put('src-tui/Cargo.toml', '[package]\nname = "fixture"\nversion = "1.0.0"\n')
  for (const path of productPaths) await put(path, '// initial\n')
  git(['init', '-b', 'release'])
  git(['config', 'user.name', 'Release fixture'])
  git(['config', 'user.email', 'fixture@example.invalid'])
  git(['config', 'commit.gpgsign', 'false'])
  git(['add', '.'])
  git(['commit', '-m', 'feat: initial product'])
  if (tags) for (const [, , prefix] of channels) git(['tag', `${prefix}1.0.0`])
  let revision = 0
  const commit = async (subject, paths = productPaths) => {
    for (const path of paths) await put(path, `// revision ${++revision}\n`)
    git(['add', '.'])
    git(['commit', '-m', subject])
  }
  const generate = async (channel, prefix) => {
    const source = git(['rev-parse', 'HEAD'])
    if (channel === 'runtime')
      await put('package.json', JSON.stringify({ type: 'module', version: '1.1.0' }))
    if (channel === 'desktop') await put('src-tauri/desktop-package.json', '{"version":"1.1.0"}')
    if (channel === 'tui')
      await put('src-tui/Cargo.toml', '[package]\nname = "fixture"\nversion = "1.1.0"\n')
    const script = channel === 'app' ? 'prepare-app-release.mjs' : 'prepare-release.mjs'
    const result = spawnSync(
      process.execPath,
      [join(root, 'scripts', script), `${prefix}1.1.0`, source, channel],
      { cwd: root, env, encoding: 'utf8' },
    )
    assert.equal(result.status, 0, result.stderr)
    const path = join(root, channel === 'app' ? 'release-body-app.md' : 'release-body.md')
    return { body: await readFile(path, 'utf8'), source, path }
  }
  return { root, env, git, put, commit, generate }
}

for (const [channel, label, prefix] of channels) {
  test(`${channel} release preparation emits the same categorized public notes contract`, async (t) => {
    const f = await fixture(t)
    await f.commit('feat(chat): 新增消息搜索')
    await f.commit('perf(ui): reduce layout work')
    await f.commit('fix: 恢复连接')
    await f.commit('refactor!: remove legacy mode')
    await f.commit('调整默认设置')
    await f.commit(`chore(release-${channel}): bookkeeping`)
    const { body, source } = await f.generate(channel, prefix)
    assert.equal(body.split('\n')[0], `## Pisper ${label} 1.1.0`)
    assert.match(body, /- 发布日期：\d{4}-\d{2}-\d{2}（北京时间）/)
    assert.ok(body.includes(`- 发布标签：\`${prefix}1.1.0\``))
    assert.deepEqual(
      [...body.matchAll(/^#{2,3} (.+)$/gm)].map((match) => match[1]),
      [
        `Pisper ${label} 1.1.0`,
        '更新内容',
        '新增功能',
        '优化改进',
        '问题修复',
        '其他变更',
        '升级说明',
        '完整变更',
      ],
    )
    assert.match(body, /### 新增功能\n\n- feat\(chat\): 新增消息搜索/)
    assert.match(body, /### 问题修复\n\n- fix: 恢复连接/)
    assert.match(body, /### 其他变更\n\n- 调整默认设置/)
    assert.doesNotMatch(body, /bookkeeping/)
    assert.match(body, /包含标记为不兼容的变更/)
    assert.ok(
      body.includes(`https://github.com/example/pisper/compare/${prefix}1.0.0...${prefix}1.1.0`),
    )
    assert.ok(body.includes(`https://github.com/example/pisper/commit/${source}`))
    if (channel === 'app') {
      const manifest = JSON.parse(await readFile(join(f.root, 'docs/latest-app.json'), 'utf8'))
      assert.equal(manifest.notes, body.trim())
      assert.match(body, /iOS.*未签名/)
    } else if (channel !== 'tui') {
      const notes = JSON.parse(await readFile(join(f.root, 'public/release-notes.json'), 'utf8'))
      assert.equal(notes.notes, body.trim())
      assert.equal(notes.component, channel)
      assert.equal(notes.version, '1.1.0')
    }
    assert.deepEqual(f.git(['tag', '--list']).split('\n').sort(), [
      'app-v1.0.0',
      'runtime-v1.0.0',
      'tui-v1.0.0',
      'v1.0.0',
    ])
  })
}

test('first release omits empty categories and links to tag history', async (t) => {
  const f = await fixture(t, { tags: false })
  const { body } = await f.generate('desktop', 'v')
  assert.match(body, /- 对比基线：首次发布/)
  assert.doesNotMatch(body, /### (优化改进|问题修复|其他变更)/)
  assert.ok(body.includes('https://github.com/example/pisper/commits/v1.1.0'))
})

test('deletion-only changes remain visible in component and App notes', async (t) => {
  const f = await fixture(t)
  await rm(join(f.root, 'src/example.ts'))
  f.git(['add', '-u'])
  f.git(['commit', '-m', 'fix: remove obsolete screen'])
  for (const channel of ['desktop', 'app']) {
    const { body } = await f.generate(channel, channel === 'app' ? 'app-v' : 'v')
    assert.match(body, /- fix: remove obsolete screen/)
  }
})

const validInput = {
  component: 'runtime',
  version: '1.1.0',
  tag: 'runtime-v1.1.0',
  previousTag: 'runtime-v1.0.0',
  sourceSha: 'a'.repeat(40),
  repository: 'example/pisper',
  date: '2026-10-14',
  subjects: ['fix: 恢复连接'],
}

test('publication validation rejects damaged bodies and mismatched release identities', () => {
  const body = formatReleaseNotes(validInput)
  assert.equal(assertReleaseNotes(body, validInput), 'Pisper Runtime 1.1.0')
  for (const invalid of [
    'unexpected prefix\n' + body,
    body.replace('2026-10-14', '2026-02-30'),
    body.replace('## 升级说明', '## 其他章节'),
    body.replace('### 问题修复', '### 未知分类'),
    body.replace('- fix: 恢复连接', ''),
    body.replace('compare/runtime-v1.0.0...runtime-v1.1.0', 'compare/v0.0.0...v9.0.0'),
  ])
    assert.throws(() => assertReleaseNotes(invalid, validInput))
  assert.throws(() =>
    assertReleaseNotes(body, { ...validInput, component: 'desktop', tag: 'v1.1.0' }),
  )
  assert.throws(() => assertReleaseNotes(body, { ...validInput, repository: 'another/project' }))
})

test('release dates use Beijing calendar dates and commit text cannot inject Markdown sections', () => {
  assert.equal(releaseNotesDate(new Date('2026-10-13T16:30:00Z')), '2026-10-14')
  const body = formatReleaseNotes({
    ...validInput,
    subjects: ['fix: <img src=x> [click](javascript:bad)\n## injected'],
  })
  assert.doesNotMatch(body, /<img|^## injected/m)
  assert.ok(body.includes('&lt;img src=x&gt;'))
  assert.ok(body.includes('\\[click\\]'))
  assertReleaseNotes(body, validInput)
})

test('non-conventional commit list markers remain literal text in Markdown and the homepage', async () => {
  const subjects = ['- remove legacy', '+ improve behavior', '1. migrate data', '2) follow up']
  const body = formatReleaseNotes({ ...validInput, subjects })
  const nestedLists = []
  const visit = (node, parent) => {
    if (node.type === 'list' && parent?.type === 'listItem') nestedLists.push(node)
    for (const child of node.children || []) visit(child, node)
  }
  visit(fromMarkdown(body), null)
  assert.equal(nestedLists.length, 0, 'commit text created a nested list')
  const source = await readFile(join(repositoryRoot, 'docs/changelog.js'), 'utf8')
  const context = vm.createContext({
    document: { querySelector: () => null, querySelectorAll: () => [] },
  })
  vm.runInContext(source + '\nglobalThis.parse = parseReleaseCommits', context)
  assert.deepEqual(
    Array.from(context.parse(body), (entry) => entry.subject),
    subjects,
  )
})

test('publication CLI accepts canonical notes and refuses a tampered artifact', async (t) => {
  const f = await fixture(t)
  const { body, path } = await f.generate('desktop', 'v')
  const run = () =>
    spawnSync(
      process.execPath,
      [join(f.root, 'scripts/validate-release-notes.mjs'), path, 'desktop', '1.1.0', 'v1.1.0'],
      { cwd: f.root, env: f.env, encoding: 'utf8' },
    )
  assert.equal(run().stdout.trim(), 'Pisper Desktop 1.1.0')
  const manifestPath = join(f.root, 'public/release-notes.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  await writeFile(manifestPath, JSON.stringify({ ...manifest, notes: 'different client notes' }))
  assert.notEqual(run().status, 0, 'client notes drift must stop publication')
  await writeFile(manifestPath, JSON.stringify(manifest))
  await writeFile(path, body.replace('## 完整变更', ''))
  assert.notEqual(run().status, 0)
})

test('homepage preserves categorized non-conventional subjects and still reads legacy notes', async () => {
  const source = await readFile(join(repositoryRoot, 'docs/changelog.js'), 'utf8')
  const context = vm.createContext({
    document: { querySelector: () => null, querySelectorAll: () => [] },
  })
  vm.runInContext(
    source + '\nglobalThis.parse = parseReleaseCommits; globalThis.category = commitCategory',
    context,
  )
  const body = formatReleaseNotes({
    ...validInput,
    subjects: ['feat: 新功能', 'perf: faster', 'fix: 恢复连接', '调整默认设置'],
  })
  const entries = context.parse(body)
  assert.equal(entries.length, 4)
  assert.deepEqual(
    Array.from(entries, (entry) => context.category(entry)),
    ['added', 'improved', 'fixed', 'other'],
  )
  assert.equal(entries[3].subject, '调整默认设置')
  assert.equal(
    context.parse("## What's Changed\n\n- fix(ui): repair pane")[0].subject,
    'repair pane',
  )
})

for (const [channel, workflow, prefix] of [
  ['desktop', 'release.yml', 'v'],
  ['app', 'release-app.yml', 'app-v'],
]) {
  test(`${channel} publication rejects invalid notes before Git or GitHub mutations`, async (t) => {
    const f = await fixture(t)
    const { path, source } = await f.generate(channel, prefix)
    await writeFile(path, 'unformatted notes')
    await mkdir(join(f.root, 'fake-bin'))
    for (const executable of ['git', 'gh']) {
      const path = join(f.root, 'fake-bin', executable)
      await writeFile(path, '#!/bin/sh\ntouch publication-mutation\nexit 93\n')
      await chmod(path, 0o755)
    }
    const config = yaml.load(
      await readFile(join(repositoryRoot, '.github/workflows', workflow), 'utf8'),
    )
    const step = Object.values(config.jobs)
      .flatMap((job) => job.steps || [])
      .find((step) => step.name?.startsWith('Commit ') && step.run?.includes('gh release create'))
    assert.ok(step)
    const result = spawnSync('bash', ['-c', step.run], {
      cwd: f.root,
      env: {
        ...f.env,
        PATH: `${join(f.root, 'fake-bin')}:${dirname(process.execPath)}:${process.env.PATH}`,
        RELEASE_COMPONENT: channel,
        RELEASE_VERSION: '1.1.0',
        RELEASE_TAG: `${prefix}1.1.0`,
        RELEASE_SOURCE_SHA: source,
        RELEASE_BRANCH: 'release',
        MAKE_LATEST: 'false',
      },
      encoding: 'utf8',
    })
    assert.notEqual(result.status, 0)
    assert.equal(
      existsSync(join(f.root, 'publication-mutation')),
      false,
      'malformed release notes reached a Git/GitHub mutation',
    )
  })
}
