import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'
import test from 'node:test'
import { GitChangesService } from '../services/git-changes-service.mjs'

const execFileAsync = promisify(execFile)

test('git changes include untracked files as green additions in the unified diff', async (t) => {
  try {
    await execFileAsync('git', ['--version'], { windowsHide: true })
  } catch {
    t.skip('Git is unavailable in this environment.')
    return
  }

  const cwd = await mkdtemp(join(tmpdir(), 'pisper-git-changes-'))
  try {
    await execFileAsync('git', ['init'], { cwd, windowsHide: true })
    await writeFile(join(cwd, 'new-file.txt'), 'first line\nsecond line\n', 'utf8')

    const changes = await new GitChangesService().getChanges(cwd)
    assert.equal(changes.isRepo, true)
    assert.deepEqual(changes.files, [{ path: 'new-file.txt', status: '??' }])
    assert.match(changes.diff, /\+\+\+ b\/new-file\.txt/)
    assert.match(changes.diff, /\+first line/)
    assert.match(changes.diff, /\+second line/)
  } finally {
    await rm(cwd, { recursive: true, force: true })
  }
})
