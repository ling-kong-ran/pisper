import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  assertHasSubstantiveReleaseCommits,
  isSubstantiveReleaseCommit,
} from '../../scripts/release-policy.mjs'

test('release validates immutable source and dispatches without versioning or tagging locally', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')
  const substantiveCheck = source.indexOf('assertHasSubstantiveReleaseCommits')
  const tests = source.indexOf("runNpm(['test'])")
  const dispatch = source.indexOf("'workflow',\n  'run',\n  'release.yml'")

  assert.ok(substantiveCheck >= 0)
  assert.ok(tests > substantiveCheck)
  assert.ok(dispatch > tests)
  assert.match(source, /runNpm\(\['run', 'check'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:test'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:check'\]\)/)
  assert.match(source, /PISPER_RELEASE_BRANCH \|\| 'release'/)
  assert.match(source, /--untracked-files=no/)
  assert.match(source, /source !== remoteSource/)
  assert.match(source, /`version=\$\{nextVersion\}`/)
  assert.match(source, /`source_sha=\$\{source\}`/)
  assert.doesNotMatch(source, /runNpm\(\['version'/)
  assert.doesNotMatch(source, /run\('git', \['tag', (?!'--list')/)
  assert.doesNotMatch(source, /run\('git', \['push'/)
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
