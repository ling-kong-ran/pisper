import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  assertHasSubstantiveReleaseCommits,
  isSubstantiveReleaseCommit,
} from '../../scripts/release-policy.mjs'

test('release refreshes npm and Cargo dependencies before checks and version tagging', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')
  const substantiveCheck = source.indexOf('assertHasSubstantiveReleaseCommits')
  const npmUpdate = source.indexOf("runNpm(['update'])")
  const cargoUpdate = source.indexOf("run('cargo', ['update'")
  const tests = source.indexOf("runNpm(['test'])")
  const dependencyCommit = source.indexOf('chore(deps): refresh release dependencies')
  const npmVersion = source.indexOf('const versionOutput')

  assert.ok(substantiveCheck >= 0)
  assert.ok(npmUpdate > substantiveCheck)
  assert.ok(cargoUpdate > npmUpdate)
  assert.ok(tests > cargoUpdate)
  assert.ok(dependencyCommit > tests)
  assert.ok(npmVersion > dependencyCommit)
  assert.match(source, /runNpm\(\['run', 'check'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:test'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:check'\]\)/)
  assert.match(source, /PISPER_RELEASE_BRANCH \|\| 'release'/)
  assert.match(source, /--untracked-files=no/)
  assert.match(source, /远端 \$\{branch\} 分支尚不存在，将在发布时创建/)
  assert.match(source, /正在检查自 \$\{latestTag\} 以来的实质性提交/)
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

  assert.throws(
    () => assertHasSubstantiveReleaseCommits([], 'v0.4.22'),
    /没有新提交，无需发布/,
  )
  assert.throws(
    () =>
      assertHasSubstantiveReleaseCommits(
        ['chore(deps): refresh release dependencies', 'chore(release): v0.4.23'],
        'v0.4.22',
      ),
    /没有实质性提交，已中止发布/,
  )
})
