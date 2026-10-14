import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { SessionFileChangesService } from '../services/session-file-changes.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { parseUnifiedDiff } from '../../src/features/chat/git-diff.ts'

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-snapshot-preview-'))
  const cwd = root
  const service = new SessionFileChangesService({ dataDir: root, warn: () => {} })
  t.after(async () => {
    await service.pruned
    await rm(root, { recursive: true, force: true })
  })
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  runtime.sessionGitCwd = async () => cwd
  runtime.getFileChangesService = () => service
  runtime.vcsChanges = { getFileDiff: async () => ({ isRepo: false, diff: '' }) }
  return { cwd, service, runtime }
}

test('snapshot preview has a single file header and parses as one changed file', async (t) => {
  const { cwd, service, runtime } = await fixture(t)
  const path = join(cwd, 'notes.txt')
  await writeFile(path, 'before\n')
  await service.run({ sessionId: 's1', cwd, name: 'write', args: { path } }, () =>
    writeFile(path, 'after\n'),
  )
  const result = await runtime.getSessionFileDiff('s1', path)
  assert.equal(result.source, 'snapshot')
  assert.equal(result.canRevert, true)
  assert.equal(result.diff.split('\n').filter((line) => line.startsWith('--- ')).length, 1)
  assert.equal(result.diff.split('\n').filter((line) => line.startsWith('+++ ')).length, 1)
  const files = parseUnifiedDiff(result.diff)
  assert.equal(files.length, 1)
  assert.equal(files[0].path, 'notes.txt')
  assert.ok(files[0].hunks.length)
})

test('restored files do not advertise a header-only diff or snapshot revert', async (t) => {
  const { cwd, service, runtime } = await fixture(t)
  const path = join(cwd, 'notes.txt')
  await writeFile(path, 'before\n')
  await service.run({ sessionId: 's1', cwd, name: 'write', args: { path } }, () =>
    writeFile(path, 'after\n'),
  )
  await service.revert('s1', cwd, 'notes.txt')
  assert.equal((await service.diff('s1', cwd, 'notes.txt')).diff, '')
  const result = await runtime.getSessionFileDiff('s1', path)
  assert.equal(result.diff, '')
  assert.notEqual(result.canRevert, true)
})
