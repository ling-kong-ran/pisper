import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('release refreshes npm and Cargo dependencies before checks and version tagging', async () => {
  const source = await readFile('scripts/release.mjs', 'utf8')
  const npmUpdate = source.indexOf("runNpm(['update'])")
  const cargoUpdate = source.indexOf("run('cargo', ['update'")
  const tests = source.indexOf("runNpm(['test'])")
  const dependencyCommit = source.indexOf('chore(deps): refresh release dependencies')
  const npmVersion = source.indexOf('const versionOutput')

  assert.ok(npmUpdate >= 0)
  assert.ok(cargoUpdate > npmUpdate)
  assert.ok(tests > cargoUpdate)
  assert.ok(dependencyCommit > tests)
  assert.ok(npmVersion > dependencyCommit)
  assert.match(source, /runNpm\(\['run', 'check'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:test'\]\)/)
  assert.match(source, /runNpm\(\['run', 'tui:check'\]\)/)
})
