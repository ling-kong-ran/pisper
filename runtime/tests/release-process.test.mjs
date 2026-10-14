import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { parse } from '@babel/parser'
import {
  assertHasSubstantiveReleaseCommits,
  isSubstantiveReleaseCommit,
} from '../../scripts/release-policy.mjs'

function parseReleaseSource(source) {
  const sourceFile = parse(source, { sourceType: 'module' })
  const updater = sourceFile.program.body.find(
    (node) => node.type === 'FunctionDeclaration' && node.id?.name === 'updateUpstreamPiDependency',
  )
  assert.ok(updater, 'release dependency updater must remain identifiable')
  const runner = sourceFile.program.body.find(
    (node) => node.type === 'FunctionDeclaration' && node.id?.name === 'run',
  )
  assert.ok(runner, 'release command runner must remain identifiable')
  return { sourceFile, updater, runner }
}

async function dependencyUpdateFixture({
  changed = 'package.json\npackage-lock.json',
  status = ' M package.json\n M package-lock.json',
  failOn = '',
} = {}) {
  const source = await readFile('scripts/release.mjs', 'utf8')
  const { updater, runner } = parseReleaseSource(source)
  const calls = []
  // 保留真实输出处理，仅替换进程边界，避免 mock 掩盖 porcelain 首列被裁剪的问题。
  const update = runInNewContext(
    `(() => {
    ${source.slice(runner.start, runner.end)}
    return ${source.slice(updater.start, updater.end)}
  })()`,
    {
      PI_CODING_AGENT_PACKAGE: '@earendil-works/pi-coding-agent',
      releaseBranch: 'release',
      root: '/unused-release-fixture',
      console: { log() {} },
      runNpm(args) {
        calls.push(['npm', ...args])
        if (failOn === 'npm') throw new Error('simulated npm failure')
      },
      execFileSync(command, args) {
        calls.push([command, ...args])
        assert.equal(command, 'git')
        if (args[0] === failOn) throw new Error(`simulated ${failOn} failure`)
        if (args[0] === 'diff') return changed
        if (args[0] === 'status') return status
        assert.ok(['add', 'commit', 'push'].includes(args[0]))
        return ''
      },
    },
  )
  return { update, calls }
}

const piInstallCommand = [
  'npm',
  'install',
  '@earendil-works/pi-coding-agent@latest',
  '--save',
  '--package-lock-only',
]

const dependencyDiffCommand = [
  'git',
  'diff',
  '--name-only',
  '--',
  'package.json',
  'package-lock.json',
]

test('release dependency update does not commit or push when manifests are unchanged', async () => {
  const { update, calls } = await dependencyUpdateFixture({ changed: '' })
  assert.equal(update(), false)
  assert.deepEqual(calls, [piInstallCommand, dependencyDiffCommand])
})

test('release dependency update commits only manifests and pushes only the release branch', async () => {
  const { update, calls } = await dependencyUpdateFixture()
  assert.equal(update(), true)
  assert.deepEqual(calls, [
    piInstallCommand,
    dependencyDiffCommand,
    ['git', 'status', '--porcelain', '--untracked-files=no'],
    ['git', 'add', 'package.json', 'package-lock.json'],
    ['git', 'commit', '-m', 'chore(deps): update pi coding agent'],
    ['git', 'push', 'origin', 'release'],
  ])
})

test('release dependency update preserves the leading status column for a single lockfile change', async () => {
  const { update, calls } = await dependencyUpdateFixture({
    changed: 'package-lock.json\n',
    status: ' M package-lock.json\n',
  })
  assert.equal(update(), true)
  assert.deepEqual(calls.at(-1), ['git', 'push', 'origin', 'release'])
})

test('release dependency update rejects unexpected tracked changes before staging or pushing', async () => {
  const { update, calls } = await dependencyUpdateFixture({
    status: ' M package.json\n M runtime/index.mjs',
  })
  assert.throws(update, /非预期修改/)
  assert.deepEqual(calls, [
    piInstallCommand,
    dependencyDiffCommand,
    ['git', 'status', '--porcelain', '--untracked-files=no'],
  ])
})

for (const failOn of ['npm', 'commit', 'push']) {
  test(`release dependency update stops after ${failOn} failure`, async () => {
    const { update, calls } = await dependencyUpdateFixture({ failOn })
    assert.throws(update, new RegExp(`simulated ${failOn} failure`))
    assert.equal(failOn === 'npm' ? calls.at(-1)[0] : calls.at(-1)[1], failOn)
    if (failOn !== 'push')
      assert.ok(!calls.some(([command, action]) => command === 'git' && action === 'push'))
  })
}

test('release validates immutable source and dispatches without versioning or tagging locally', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')
  const substantiveCheck = source.indexOf('subjects.filter(isSubstantiveReleaseCommit)')
  const checks = source.indexOf('runComponentChecks(selectedComponents)')
  const dispatch = source.indexOf("'workflow'")

  assert.ok(substantiveCheck >= 0)
  assert.ok(checks > substantiveCheck)
  assert.ok(dispatch > checks)
  assert.match(source, /runNpm\(\['run', 'postinstall'\]\)/)
  assert.match(source, /runNpm\(\['test'\]\)/)
  assert.match(source, /runNpm\(\['run', 'check'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:test'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:check'\]\)/)
  assert.match(source, /PISPER_RELEASE_BRANCH \|\| 'release'/)
  assert.match(source, /--untracked-files=no/)
  assert.match(source, /source !== remoteSource/)
  assert.match(source, /`version=\$\{nextVersion\}`/)
  assert.match(source, /`source_sha=\$\{source\}`/)
  assert.match(source, /run\('gh', \['run', 'watch', runId, '--exit-status'\]\)/)
  assert.doesNotMatch(source, /index < plans\.length - 1/)
  assert.match(source, /component === 'app' \? 'release-app\.yml' : 'release\.yml'/)
  assert.doesNotMatch(source, /runNpm\(\['version'/)
  assert.doesNotMatch(source, /run\('git', \['tag', (?!'--list')/)
  const { sourceFile, updater } = parseReleaseSource(source)
  const mutations = []
  const visit = (node) => {
    if (
      node.type === 'CallExpression' &&
      node.callee.type === 'Identifier' &&
      node.callee.name === 'run' &&
      node.arguments[0]?.type === 'StringLiteral' &&
      node.arguments[0].value === 'git' &&
      node.arguments[1]?.type === 'ArrayExpression'
    ) {
      const command = node.arguments[1].elements[0]
      if (command?.type === 'StringLiteral' && ['add', 'commit', 'push'].includes(command.value)) {
        assert.ok(node.start >= updater.start && node.end <= updater.end)
        mutations.push(command.value)
      }
    }
    for (const value of Object.values(node)) {
      for (const child of Array.isArray(value) ? value : [value]) {
        if (child && typeof child === 'object' && typeof child.type === 'string') visit(child)
      }
    }
  }
  visit(sourceFile)
  assert.deepEqual(mutations, ['add', 'commit', 'push'])
  const updateCall = source.indexOf('\nupdateUpstreamPiDependency()')
  const updatedSource = source.indexOf("source = run('git', ['rev-parse', 'HEAD']", updateCall)
  const updatedRemote = source.indexOf('remoteSource = run(', updateCall)
  const synchronized = source.indexOf('if (source !== remoteSource)', updatedRemote)
  assert.ok(updateCall >= 0 && updatedSource > updateCall && updatedRemote > updatedSource)
  assert.ok(synchronized > updatedRemote && synchronized < substantiveCheck)
  // 依赖同步可以在派发前推送；版本文件和标签仍由远端 workflow 原子更新。
  // Desktop 派发继续携带同批次最新 TUI/Runtime 版本。
  assert.match(source, /安装包自动链式发布 desktop/)
  assert.match(source, /desktopTuiVersion \|\| desktopRuntimeVersion/)
  assert.match(source, /`tui_version=\$\{desktopTuiVersion\}`/)
})

test('release pre-checks verify lockfile integrity against the registry before installing', async () => {
  // 手写损坏的 integrity 只会在冷 `npm ci` 时爆发，必须在派发前的本地检查里拦住。
  const source = await readFile('scripts/release.mjs', 'utf8')
  const guard = source.indexOf("'scripts/verify-lockfile-integrity.mjs'")
  const postinstall = source.indexOf("runNpm(['run', 'postinstall'])")
  assert.ok(guard >= 0)
  assert.ok(postinstall > guard)

  const script = await readFile('scripts/verify-lockfile-integrity.mjs', 'utf8')
  assert.match(script, /https:\/\/registry\.npmjs\.org\//)
  assert.match(script, /dist\.integrity/)
})

test('desktop staging treats a component bundle refresh as a substantive change', async () => {
  const stage = await readFile('scripts/stage-release-version.mjs', 'utf8')
  assert.match(stage, /RELEASE_TUI_VERSION/)
  assert.match(stage, /RELEASE_RUNTIME_VERSION/)
  assert.match(stage, /bundled\.tui = tuiVersion/)
  assert.match(stage, /component !== 'desktop' \|\| \(!tuiVersion && !runtimeVersion\)/)
})

test('the npm lockfile preserves third-party package registry identities', async () => {
  const lockfile = await readFile('package-lock.json', 'utf8')

  assert.match(lockfile, /@hono\/node-server\/-\/node-server-2\.1\.0\.tgz/)
  assert.doesNotMatch(lockfile, /@hono\/node-runtime/)
})

test('queued component releases accept only isolated version commits after the source', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-release-head-'))
  const git = (args) =>
    spawnSync('git', args, {
      cwd: directory,
      encoding: 'utf8',
      env: { ...process.env, GIT_CONFIG_NOSYSTEM: '1' },
    })

  try {
    assert.equal(git(['init']).status, 0)
    assert.equal(git(['config', 'user.name', 'Pisper Tests']).status, 0)
    assert.equal(git(['config', 'user.email', 'tests@pisper.local']).status, 0)
    await writeFile(join(directory, 'package.json'), '{"version":"1.0.0"}\n')
    await writeFile(join(directory, 'package-lock.json'), '{"version":"1.0.0"}\n')
    assert.equal(git(['add', '.']).status, 0)
    assert.equal(git(['commit', '-m', 'feat(runtime): initial runtime']).status, 0)
    const source = git(['rev-parse', 'HEAD']).stdout.trim()

    await writeFile(join(directory, 'package.json'), '{"version":"1.0.1"}\n')
    await writeFile(join(directory, 'package-lock.json'), '{"version":"1.0.1"}\n')
    assert.equal(git(['add', 'package.json', 'package-lock.json']).status, 0)
    assert.equal(git(['commit', '-m', 'chore(release-runtime): runtime-v1.0.1']).status, 0)
    const releaseHead = git(['rev-parse', 'HEAD']).stdout.trim()
    const valid = spawnSync(
      process.execPath,
      ['scripts/verify-release-head.mjs', source, releaseHead],
      {
        cwd: process.cwd(),
        encoding: 'utf8',
        env: { ...process.env, PISPER_RELEASE_ROOT: directory },
      },
    )
    assert.equal(valid.status, 0, valid.stderr)

    assert.equal(git(['switch', '--create', 'queued-tui', source]).status, 0)
    await mkdir(join(directory, 'src-tui'))
    await writeFile(join(directory, 'src-tui', 'Cargo.toml'), '[package]\nversion = "1.0.1"\n')
    await writeFile(join(directory, 'src-tui', 'Cargo.lock'), 'version = "1.0.1"\n')
    assert.equal(git(['add', 'src-tui/Cargo.toml', 'src-tui/Cargo.lock']).status, 0)
    assert.equal(git(['commit', '-m', 'chore(release-tui): tui-v1.0.1']).status, 0)
    assert.equal(git(['rebase', '--onto', releaseHead, source, 'HEAD']).status, 0)
    const queuedHead = git(['rev-parse', 'HEAD']).stdout.trim()
    const queued = spawnSync(
      process.execPath,
      ['scripts/verify-release-head.mjs', source, queuedHead],
      {
        cwd: process.cwd(),
        encoding: 'utf8',
        env: { ...process.env, PISPER_RELEASE_ROOT: directory },
      },
    )
    assert.equal(queued.status, 0, queued.stderr)
    assert.match(queued.stdout, /2 validated component version commit/)

    await writeFile(join(directory, 'runtime.mjs'), 'export const changed = true\n')
    assert.equal(git(['add', 'runtime.mjs']).status, 0)
    assert.equal(git(['commit', '-m', 'fix(runtime): mutate queued source']).status, 0)
    const invalidHead = git(['rev-parse', 'HEAD']).stdout.trim()
    const invalid = spawnSync(
      process.execPath,
      ['scripts/verify-release-head.mjs', source, invalidHead],
      {
        cwd: process.cwd(),
        encoding: 'utf8',
        env: { ...process.env, PISPER_RELEASE_ROOT: directory },
      },
    )
    assert.notEqual(invalid.status, 0)
    assert.match(invalid.stderr, /non-release commit/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('release policy accepts product commits and rejects bookkeeping-only ranges', () => {
  assert.equal(isSubstantiveReleaseCommit('feat(chat): remember session model'), true)
  assert.equal(isSubstantiveReleaseCommit('fix(storage): harden Windows atomic writes'), true)
  assert.equal(isSubstantiveReleaseCommit('perf(ui): reduce WebView overhead'), true)
  assert.equal(isSubstantiveReleaseCommit('refactor(runtime): compose agent domains'), true)
  assert.equal(isSubstantiveReleaseCommit('build(sea): prune runtime closure'), true)
  assert.equal(isSubstantiveReleaseCommit('security: redact credentials'), true)
  assert.equal(isSubstantiveReleaseCommit('revert: undo broken model picker'), true)
  assert.equal(isSubstantiveReleaseCommit('chore(deps): refresh release dependencies'), false)
  assert.equal(isSubstantiveReleaseCommit('chore(release): v0.4.22'), false)
  assert.equal(isSubstantiveReleaseCommit('style(runtime): format restore call'), false)
  assert.equal(isSubstantiveReleaseCommit('docs: expand repository command guide'), false)
  assert.equal(isSubstantiveReleaseCommit('test: cover release policy'), false)
  assert.equal(isSubstantiveReleaseCommit('ci: bridge Electron users into Tauri release'), false)

  assert.deepEqual(
    assertHasSubstantiveReleaseCommits(
      ['fix(chat): persist model switches', 'chore(deps): refresh release dependencies'],
      'v0.4.21',
    ),
    ['fix(chat): persist model switches'],
  )

  assert.throws(() => assertHasSubstantiveReleaseCommits([], 'v0.4.22'), /没有新提交，无需发布/)
  assert.throws(
    () =>
      assertHasSubstantiveReleaseCommits(
        ['chore(deps): refresh release dependencies', 'chore(release): v0.4.23'],
        'v0.4.22',
      ),
    /没有实质性提交，已中止发布/,
  )
})
