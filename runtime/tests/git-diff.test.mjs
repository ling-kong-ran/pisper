import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { parseUnifiedDiff } from '../../src/features/chat/git-diff.ts'

const SAMPLE = `diff --git a/src/example.ts b/src/example.ts
index 1111111..2222222 100644
--- a/src/example.ts
+++ b/src/example.ts
@@ -1,3 +1,4 @@
 const value = 1
-oldCall()
+newCall()
+extraCall()
 export default value
diff --git a/removed.txt b/removed.txt
deleted file mode 100644
index 3333333..0000000
--- a/removed.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-first
-second
`

test('unified git diff parses into aligned original and modified rows', () => {
  const files = parseUnifiedDiff(SAMPLE)
  assert.equal(files.length, 2)
  assert.equal(files[0].path, 'src/example.ts')

  const rows = files[0].hunks[0].rows.filter((row) => row.kind === 'pair')
  assert.deepEqual(rows[0], {
    kind: 'pair',
    old: { lineNumber: 1, text: 'const value = 1', tone: 'context' },
    next: { lineNumber: 1, text: 'const value = 1', tone: 'context' },
  })
  assert.deepEqual(rows[1], {
    kind: 'pair',
    old: { lineNumber: 2, text: 'oldCall()', tone: 'deleted' },
    next: { lineNumber: 2, text: 'newCall()', tone: 'added' },
  })
  assert.deepEqual(rows[2], {
    kind: 'pair',
    old: { lineNumber: null, text: '', tone: 'empty' },
    next: { lineNumber: 3, text: 'extraCall()', tone: 'added' },
  })
  assert.deepEqual(rows[3], {
    kind: 'pair',
    old: { lineNumber: 3, text: 'export default value', tone: 'context' },
    next: { lineNumber: 4, text: 'export default value', tone: 'context' },
  })
})

test('deleted files keep red original rows and empty modified rows', () => {
  const removed = parseUnifiedDiff(SAMPLE)[1]
  assert.equal(removed.path, 'removed.txt')
  assert.match(removed.metadata.join(' '), /deleted file mode/)
  const rows = removed.hunks[0].rows.filter((row) => row.kind === 'pair')
  assert.equal(rows.length, 2)
  assert.equal(rows[0].old.tone, 'deleted')
  assert.equal(rows[0].next.tone, 'empty')
  assert.equal(rows[1].old.lineNumber, 2)
})

test('diff dialog navigates files separately and renders one side-by-side diff', async () => {
  const [viewer, approval] = await Promise.all([
    readFile('src/features/chat/GitDiffViewer.tsx', 'utf8'),
    readFile('src/features/chat/ToolApproval.tsx', 'utf8'),
  ])
  assert.match(viewer, /className="git-diff-file-nav-list[^"\n]*"/)
  assert.match(viewer, /selectedEntry && \(/)
  assert.match(
    viewer,
    /<section[\s\S]*?className="min-w-\[880px\][^"\n]*"[\s\S]*?key=\{selectedEntry\.file\.path\}/,
  )
  assert.doesNotMatch(viewer, /files\.map\(\(file/)
  assert.match(viewer, /useVirtualizer<HTMLDivElement, HTMLDivElement>/)
  assert.match(viewer, /data-pisper-diff-row-count=\{items\.length\}/)
  assert.match(viewer, /data-pisper-rendered-count=\{virtualItems\.length\}/)
  assert.match(viewer, /<DiffCell side="old" cell=\{item\.row\.old\} \/>/)
  assert.match(viewer, /<DiffCell side="next" cell=\{item\.row\.next\} \/>/)
  assert.match(viewer, /git-diff-workbench[^"\n]*grid-cols-/)
  assert.match(viewer, /className="min-w-0 min-h-0 overflow-auto/)
  assert.match(viewer, /className="relative min-w-\[880px\] w-full"/)
  assert.match(viewer, /className="absolute left-0 w-full"/)
  assert.match(viewer, /className="grid min-w-\[880px\] grid-cols-/)
  assert.match(viewer, /git-diff-cell[^`\n]*\[&\.deleted\]:bg-\[var\(--danger-soft\)\]/)
  assert.match(viewer, /git-diff-cell[^`\n]*\[&\.added\]:bg-\[var\(--success-soft\)\]/)
  assert.match(approval, /const fileChange = approval\.fileChange/)
  assert.match(approval, /<GitDiffDialog/)
  assert.match(approval, /className="tool-approval-view-diff[^"\n]*"/)
})
