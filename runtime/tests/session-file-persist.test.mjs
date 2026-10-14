import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ensureSessionFilePersisted } from '../runtime/session-file-persist.mjs'

test('concurrent materialization of one session cannot overwrite its first file', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-persist-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const file = join(directory, 'session.jsonl')
  let synchronized = 0
  const manager = {
    sessionFile: file,
    getSessionId: () => 'session-1',
    getCwd: () => directory,
    setSessionFile(path) {
      assert.equal(path, file)
      synchronized += 1
    },
  }

  await Promise.all([
    ensureSessionFilePersisted(manager, 'First title', directory),
    ensureSessionFilePersisted(manager, 'Second title', directory),
  ])
  const entries = (await readFile(file, 'utf8'))
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line))
  assert.deepEqual(
    entries.map((entry) => entry.type),
    ['session', 'session_info'],
  )
  assert.equal(entries[1].name, 'First title')
  assert.equal(synchronized, 1)
  await ensureSessionFilePersisted(manager, 'Third title', directory)
  assert.equal(synchronized, 1)
})
