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

test('the file chip prefers the session snapshot while the workspace view keeps the git diff', async (t) => {
  const { cwd, service, runtime } = await fixture(t)
  const gitDiff =
    'diff --git a/notes.txt b/notes.txt\n--- a/notes.txt\n+++ b/notes.txt\n@@ git @@\n'
  runtime.vcsChanges = { getFileDiff: async () => ({ isRepo: true, vcs: 'git', diff: gitDiff }) }
  const path = join(cwd, 'notes.txt')
  await writeFile(path, 'before\n')
  await service.run({ sessionId: 's1', cwd, name: 'write', args: { path } }, () =>
    writeFile(path, 'after\n'),
  )
  const workspace = await runtime.getSessionFileDiff('s1', path)
  assert.equal(workspace.source, 'git')
  assert.equal(workspace.diff, gitDiff)
  assert.equal(workspace.canRevert, false)

  const chip = await runtime.getSessionFileDiff('s1', path, { preferSession: true })
  assert.equal(chip.source, 'snapshot')
  assert.equal(chip.canRevert, true)
  assert.match(chip.diff, /\+after/)

  // 没有快照（如 shell 生成）或已经撤销时退回 git diff，并且不提供撤销。
  const generated = join(cwd, 'generated.txt')
  await writeFile(generated, 'output\n')
  const untracked = await runtime.getSessionFileDiff('s1', generated, { preferSession: true })
  assert.equal(untracked.source, 'git')
  assert.equal(untracked.canRevert, false)
  await service.revert('s1', cwd, 'notes.txt')
  const restored = await runtime.getSessionFileDiff('s1', path, { preferSession: true })
  assert.equal(restored.source, 'git')
  assert.equal(restored.canRevert, false)
})
