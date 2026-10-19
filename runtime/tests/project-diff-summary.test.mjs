import assert from 'node:assert/strict'
import test from 'node:test'
import { summarizeProjectDiff } from '../../src/features/chat/project-diff-summary.ts'
test('text totals count lines, preserve filenames, and flag incomplete project diffs', () => {
  const diff =
    'diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -1 +1,2 @@\n-old\n+new\n+extra\n'
  assert.deepEqual(summarizeProjectDiff(diff, ['a.ts'], false), {
    added: 2,
    removed: 1,
    partial: false,
    files: { 'a.ts': { added: 2, removed: 1 } },
  })
  assert.equal(summarizeProjectDiff(diff, ['a.ts', 'image.png'], false).partial, true)
  assert.equal(summarizeProjectDiff(diff, ['a.ts'], true).partial, true)
})
