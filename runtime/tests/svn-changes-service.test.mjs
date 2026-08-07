import assert from 'node:assert/strict'
import test from 'node:test'
import { parseUnifiedDiff } from '../../src/features/chat/git-diff.ts'
import { normalizeSvnDiff, parseSvnStatusXml } from '../services/svn-changes-service.mjs'
import { VcsChangesService } from '../services/vcs-changes-service.mjs'

test('svn status xml parses into change entries', () => {
  const xml = `<?xml version="1.0" encoding="UTF-8"?>
<status>
  <target path=".">
    <entry path="src/app.ts">
      <wc-status item="modified" props="none" revision="12"></wc-status>
    </entry>
    <entry path="docs/new file &amp; notes.md">
      <wc-status item="unversioned" props="none"></wc-status>
    </entry>
    <entry path="src/old.ts">
      <wc-status item="deleted" props="none" revision="9"></wc-status>
    </entry>
    <entry path="src/missing.ts">
      <wc-status item="missing" props="none"></wc-status>
    </entry>
    <entry path="src/conflict.ts">
      <wc-status item="conflicted" props="none"></wc-status>
    </entry>
    <entry path="vendor/lib">
      <wc-status item="external" props="none"></wc-status>
    </entry>
  </target>
</status>`
  assert.deepEqual(parseSvnStatusXml(xml), [
    { path: 'src/app.ts', status: 'M' },
    { path: 'docs/new file & notes.md', status: '?' },
    { path: 'src/old.ts', status: 'D' },
    { path: 'src/missing.ts', status: '!' },
    { path: 'src/conflict.ts', status: 'C' },
  ])
})

test('svn diff normalizes into git-style unified diff the viewer can parse', () => {
  const svnDiff = `Index: src/app.ts
===================================================================
--- src/app.ts\t(revision 12)
+++ src/app.ts\t(working copy)
@@ -1,3 +1,4 @@
 const value = 1
-oldCall()
+newCall()
+extraCall()
 export default value
Index: src/old.ts
===================================================================
--- src/old.ts\t(revision 9)
+++ src/old.ts\t(nonexistent)
@@ -1,2 +0,0 @@
-first
-second
Index: added.txt
===================================================================
--- added.txt\t(revision 0)
+++ added.txt\t(working copy)
@@ -0,0 +1,1 @@
+fresh
`

  const normalized = normalizeSvnDiff(svnDiff)
  assert.match(normalized, /^diff --git a\/src\/app\.ts b\/src\/app\.ts/m)
  assert.match(normalized, /^--- a\/src\/app\.ts$/m)
  assert.match(normalized, /^\+\+\+ b\/src\/app\.ts$/m)
  assert.match(normalized, /^\+\+\+ \/dev\/null$/m)
  assert.match(normalized, /^--- \/dev\/null$/m)

  const files = parseUnifiedDiff(normalized)
  assert.equal(files.length, 3)
  assert.equal(files[0].path, 'src/app.ts')
  const removedRows = files[1].hunks[0].rows
  assert.equal(files[1].path, 'src/old.ts')
  assert.ok(removedRows.some((row) => row.old?.tone === 'deleted'))
  const addedRows = files[2].hunks[0].rows
  assert.equal(files[2].path, 'added.txt')
  assert.ok(addedRows.some((row) => row.next?.tone === 'added'))
})

test('svn diff preserves paths containing spaces', () => {
  const normalized = normalizeSvnDiff(`Index: docs/new file.md
===================================================================
--- docs/new file.md\t(revision 1)
+++ docs/new file.md\t(working copy)
@@ -1 +1 @@
-old
+new
`)
  assert.match(normalized, /^diff --git "a\/docs\/new file\.md" "b\/docs\/new file\.md"$/m)
  const files = parseUnifiedDiff(normalized)
  assert.equal(files.length, 1)
  assert.equal(files[0].path, 'docs/new file.md')
})

test('svn diff normalization drops property-change blocks', () => {
  const svnDiff = `Index: src/app.ts
===================================================================
--- src/app.ts\t(revision 3)
+++ src/app.ts\t(working copy)

Property changes on: src/app.ts
___________________________________________________________________
Added: svn:mergeinfo
   Merged /trunk/src/app.ts:r2-3
Index: src/other.ts
===================================================================
--- src/other.ts\t(revision 1)
+++ src/other.ts\t(working copy)
@@ -1 +1 @@
-a
+b
`
  const normalized = normalizeSvnDiff(svnDiff)
  assert.ok(!normalized.includes('Property changes'))
  assert.ok(!normalized.includes('svn:mergeinfo'))
  const files = parseUnifiedDiff(normalized)
  assert.equal(files.length, 2)
  assert.equal(files[1].path, 'src/other.ts')
})

test('vcs changes service prefers git repositories and falls back to svn', async () => {
  const gitChanges = { vcs: 'git', isRepo: true, files: [], diff: '' }
  const svnChanges = { vcs: 'svn', isRepo: true, files: [], diff: '' }
  const notRepo = { vcs: '', isRepo: false, files: [], diff: '', error: '' }

  const gitFirst = new VcsChangesService({
    git: {
      async getChanges() {
        return gitChanges
      },
    },
    svn: {
      async getChanges() {
        throw new Error('svn must not run when git matches')
      },
    },
  })
  assert.equal((await gitFirst.getChanges('/repo')).vcs, 'git')

  const svnFallback = new VcsChangesService({
    git: {
      async getChanges() {
        return { ...notRepo, gitAvailable: true }
      },
    },
    svn: {
      async getChanges() {
        return svnChanges
      },
    },
  })
  assert.equal((await svnFallback.getChanges('/wc')).vcs, 'svn')

  const neither = new VcsChangesService({
    git: {
      async getChanges() {
        return { ...notRepo, gitAvailable: true }
      },
    },
    svn: {
      async getChanges() {
        return { ...notRepo, svnAvailable: false }
      },
    },
  })
  const empty = await neither.getChanges('/nowhere')
  assert.equal(empty.vcs, '')
  assert.equal(empty.isRepo, false)
  assert.equal(empty.svnAvailable, false)

  const svnCommit = new VcsChangesService({
    git: {
      async getChanges() {
        return { ...notRepo, gitAvailable: true }
      },
    },
    svn: {
      async getChanges() {
        return svnChanges
      },
      async commit(cwd, message) {
        return { ...svnChanges, committed: message }
      },
    },
  })
  assert.deepEqual(await svnCommit.commit('/wc', 'msg'), { ...svnChanges, committed: 'msg' })

  const svnPush = new VcsChangesService({
    git: {
      async getChanges() {
        return { ...notRepo, gitAvailable: true }
      },
    },
    svn: {
      async getChanges() {
        return svnChanges
      },
    },
  })
  await assert.rejects(() => svnPush.push('/wc'), /无需推送/)

  await assert.rejects(() => neither.commit('/nowhere', 'msg'), /Git\/SVN/)
})
