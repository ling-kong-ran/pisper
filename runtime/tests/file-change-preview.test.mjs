import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { parseUnifiedDiff } from '../../src/features/chat/git-diff.ts'
import { createFileChangePreview, sameFileChangeSource } from '../services/file-change-preview.mjs'

test('file change previews use an in-memory unified diff for edit operations', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-file-change-preview-'))
  const path = join(directory, 'example.ts')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, 'const value = 1\r\nexport { value }\r\n', 'utf8')

  const preview = await createFileChangePreview({
    cwd: directory,
    toolName: 'edit',
    args: {
      path: 'example.ts',
      edits: [{ oldText: 'const value = 1', newText: 'const value = 2' }],
    },
  })

  assert.equal(preview.path, 'example.ts')
  assert.equal(preview.truncated, false)
  assert.match(preview.diff, /^diff --git a\/example\.ts b\/example\.ts$/m)
  const [file] = parseUnifiedDiff(preview.diff)
  assert.equal(file.path, 'example.ts')
  const rows = file.hunks[0].rows.filter((row) => row.kind === 'pair')
  assert.equal(rows[0].old.text, 'const value = 1')
  assert.equal(rows[0].next.text, 'const value = 2')
  assert.equal(await readFile(path, 'utf8'), 'const value = 1\r\nexport { value }\r\n')
})

test('write previews show new files without creating them', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-file-change-write-preview-'))
  const path = join(directory, 'nested', 'new file.md')
  t.after(() => rm(directory, { recursive: true, force: true }))

  const preview = await createFileChangePreview({
    cwd: directory,
    toolName: 'write',
    args: { path: 'nested/new file.md', content: '# Draft\n' },
  })

  assert.equal(preview.path, 'nested/new file.md')
  assert.equal(preview.sourceExists, false)
  assert.match(preview.diff, /^diff --git "a\/nested\/new file\.md" "b\/nested\/new file\.md"$/m)
  assert.match(preview.diff, /^new file mode 100644$/m)
  const [file] = parseUnifiedDiff(preview.diff)
  assert.equal(file.path, 'nested/new file.md')
  assert.equal(file.oldPath, '')
  assert.equal(file.newPath, 'nested/new file.md')
  await assert.rejects(readFile(path, 'utf8'), { code: 'ENOENT' })
})

test('file source fingerprints invalidate a preview after an external change', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-file-change-fingerprint-'))
  const path = join(directory, 'example.txt')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await writeFile(path, 'before\n', 'utf8')
  const first = await createFileChangePreview({
    cwd: directory,
    toolName: 'write',
    args: { path: 'example.txt', content: 'after\n' },
  })
  await writeFile(path, 'changed elsewhere\n', 'utf8')
  const second = await createFileChangePreview({
    cwd: directory,
    toolName: 'write',
    args: { path: 'example.txt', content: 'after\n' },
  })

  assert.equal(sameFileChangeSource(first, second), false)
})
