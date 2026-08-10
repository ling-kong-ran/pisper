import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  assertHasSubstantiveReleaseCommits,
  isSubstantiveReleaseCommit,
} from '../../scripts/release-policy.mjs'

test('release validates immutable source and dispatches without versioning or tagging locally', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')
  const substantiveCheck = source.indexOf('subjects.filter(isSubstantiveReleaseCommit)')
  const checks = source.indexOf('runComponentChecks(selectedComponents)')
  const dispatch = source.indexOf("'workflow'")

  assert.ok(substantiveCheck >= 0)
  assert.ok(checks > substantiveCheck)
  assert.ok(dispatch > checks)
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
  assert.match(source, /index < plans\.length - 1/)
  assert.doesNotMatch(source, /runNpm\(\['version'/)
  assert.doesNotMatch(source, /run\('git', \['tag', (?!'--list')/)
  assert.doesNotMatch(source, /run\('git', \['push'/)
  // A TUI/Runtime release chains the desktop installer (it bundles the newest
  // published components); the desktop dispatch carries their versions and
  // the local script never bumps versions or pushes.
  assert.match(source, /安装包自动链式发布 desktop/)
  assert.match(source, /desktopTuiVersion \|\| desktopRuntimeVersion/)
  assert.match(source, /`tui_version=\$\{desktopTuiVersion\}`/)
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
