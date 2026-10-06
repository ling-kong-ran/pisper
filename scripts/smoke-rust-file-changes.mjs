import assert from 'node:assert/strict'
import { createHash, randomUUID } from 'node:crypto'
import { access, mkdir, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { basename, dirname, join, relative, resolve } from 'node:path'

const zeroSummary = {
  status: 'known',
  changedFiles: 0,
  pendingFiles: 0,
  added: 0,
  removed: 0,
  unknownFiles: 0,
  capped: false,
}

// 所有 HTTP、原文文件与模拟旧索引都属于调用方的隔离夹具。
export async function checkFileChangeParity({
  check,
  json,
  request,
  chat,
  workspace,
  agent,
  providerId,
  modelId,
  delay,
  waitForHeldModel,
  heldModelRequests,
}) {
  const cwd = resolve(workspace)
  const data = resolve(agent)
  const workspaceRoot = await realpath(cwd)
  const snapshotRoot = join(data, 'file-change-snapshots')
  let retained

  const base = (id) => `/api/sessions/${id}`
  const changes = (id) => `${base(id)}/file-changes`
  const summary = (id) => `${base(id)}/change-summary`
  const snapshotDir = (id) =>
    join(snapshotRoot, createHash('sha256').update(id).digest('hex').slice(0, 32))
  const prompt = (name, args) =>
    'rust-snapshot-tool:' + Buffer.from(JSON.stringify({ name, args })).toString('base64')

  function response(path, method, body) {
    return request(path, {
      method,
      ...(body === undefined
        ? {}
        : { headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }),
      signal: AbortSignal.timeout(15000),
    })
  }
  async function poll(read, predicate, label, timeout = 10000) {
    const deadline = Date.now() + timeout
    let last
    while (Date.now() < deadline) {
      last = await read()
      if (predicate(last)) return last
      await delay(25)
    }
    assert.fail(`${label} did not settle: ${JSON.stringify(last)}`)
  }
  async function error(path, method, body, status, code) {
    const result = await response(path, method, body)
    const value = await result.json()
    assert.equal(result.status, status, JSON.stringify(value))
    assert.equal(value.code, code)
    assert.equal(typeof value.error, 'string')
  }
  async function absent(path) {
    await assert.rejects(access(path), (error) => error.code === 'ENOENT')
  }
  async function folder() {
    const dir = join(cwd, `file-change-fixture-${randomUUID()}`)
    await mkdir(dir)
    return dir
  }
  async function removeFolder(dir) {
    // 递归清理前核验真实绝对路径，只接受本脚本在工作区下创建的直接子目录。
    const actual = await realpath(dir)
    assert.equal(dirname(actual), workspaceRoot)
    assert.ok(basename(actual).startsWith('file-change-fixture-'))
    await rm(actual, { recursive: true })
  }
  async function session(name) {
    const value = await json('/api/sessions', 'POST', { name, cwd })
    assert.ok(value.id)
    try {
      await json(`${base(value.id)}/model`, 'PUT', { provider: providerId, model: modelId })
      await json(`${base(value.id)}/execution-mode`, 'PUT', { mode: 'full-access' })
    } catch (failure) {
      try {
        await removeSession(value.id)
      } catch (cleanup) {
        throw new AggregateError([failure, cleanup], 'Session setup and owned cleanup failed')
      }
      throw failure
    }
    return value
  }
  async function removeSession(id) {
    const aborted = await response(`${base(id)}/abort`, 'POST', {})
    assert.ok([200, 404].includes(aborted.status), `Owned session abort: ${aborted.status}`)
    await poll(
      async () => {
        const result = await response(base(id), 'DELETE')
        if (result.status === 404) return true
        if (result.status === 409) return false
        const value = await result.json()
        assert.equal(result.status, 200, JSON.stringify(value))
        assert.equal(value.deleted, true)
        return true
      },
      Boolean,
      `Delete owned snapshot session ${id}`,
      5000,
    )
  }
  async function withSession(name, run) {
    let owned
    let dir
    let result
    const failures = []
    try {
      dir = await folder()
      owned = await session(name)
      result = await run(owned.id, dir, relative(cwd, dir).replaceAll('\\', '/'))
    } catch (failure) {
      failures.push(failure)
    } finally {
      if (owned) {
        try {
          await removeSession(owned.id)
        } catch (failure) {
          failures.push(failure)
        }
      }
      if (dir) {
        try {
          await removeFolder(dir)
        } catch (failure) {
          failures.push(failure)
        }
      }
    }
    if (failures.length) throw new AggregateError(failures, failures.map(String).join('\n'))
    return result
  }

  await check('New sessions persist complete empty file-change markers', () =>
    withSession('snapshot-empty-fixture', async (id) => {
      assert.deepEqual(await json(summary(id)), zeroSummary)
      const list = await json(changes(id))
      assert.equal(resolve(list.cwd), cwd)
      assert.deepEqual(list.files, [])
      assert.deepEqual(list.summary, { files: 0, pending: 0, added: 0, removed: 0 })
      const index = JSON.parse(await readFile(join(snapshotDir(id), 'index.json'), 'utf8'))
      assert.equal(index.version, 2)
      assert.equal(index.coverage, 'complete')
      assert.equal(resolve(index.cwd), cwd)
      assert.deepEqual(index.entries, [])
      return { sessionId: id, marker: 'index-v2', status: 'known' }
    }),
  )

  await check('Actual Pi write/edit preserve the first baseline, diff and approval state', () =>
    withSession('snapshot-write-edit-fixture', async (id, dir, prefix) => {
      const path = `${prefix}/original.txt`
      const original = 'one\ntwo\nthree\n'
      await writeFile(join(dir, 'original.txt'), original)
      const writes = await chat(prompt('write', { path, content: 'one\nTWO\nthree\nfour\n' }), id)
      assert.ok(writes.some((event) => event.event === 'tool_start' && event.data.name === 'write'))
      assert.equal(await readFile(join(dir, 'original.txt'), 'utf8'), 'one\nTWO\nthree\nfour\n')
      const first = await json(changes(id))
      assert.equal(first.files.length, 1)
      assert.equal(first.files[0].path, path)
      assert.equal(first.files[0].status, 'modified')
      assert.equal(first.files[0].snapshot, true)
      assert.equal(first.files[0].canRevert, true)
      assert.equal(first.files[0].changeCount, 1)
      assert.deepEqual(first.summary, { files: 1, pending: 1, added: 2, removed: 1 })
      const diff = await json(`${changes(id)}/diff?path=${encodeURIComponent(path)}`)
      assert.equal(diff.source, 'snapshot')
      assert.equal(diff.found, true)
      assert.equal(diff.diffTruncated, false)
      assert.ok(diff.diff.includes(`--- a/${path}\n`))
      assert.ok(diff.diff.includes('+TWO\n'))
      const approved = await json(`${changes(id)}/approve`, 'POST', { path })
      assert.equal(approved.files[0].approved, true)
      assert.equal(approved.summary.pending, 0)
      assert.equal((await json(summary(id))).pendingFiles, 0)
      const edits = await chat(prompt('edit', { path, oldText: 'TWO', newText: 'AGAIN' }), id)
      assert.ok(edits.some((event) => event.event === 'tool_start' && event.data.name === 'edit'))
      assert.equal(await readFile(join(dir, 'original.txt'), 'utf8'), 'one\nAGAIN\nthree\nfour\n')
      const second = await json(changes(id))
      assert.equal(second.files[0].changeCount, 2)
      assert.equal(second.files[0].approved, false)
      assert.equal(second.summary.pending, 1)
      const reverted = await json(`${changes(id)}/revert`, 'POST', { path })
      assert.equal(reverted.reverted, 1)
      assert.equal(reverted.summary.pending, 0)
      assert.equal(await readFile(join(dir, 'original.txt'), 'utf8'), original)
      assert.deepEqual(await json(summary(id)), zeroSummary)
      return { sessionId: id, actualTools: ['write', 'edit'], firstBaselineRestored: true }
    }),
  )

  await check('New-file diff uses /dev/null and actual revert removes the created file', () =>
    withSession('snapshot-created-fixture', async (id, dir, prefix) => {
      const path = `${prefix}/created.md`
      await chat(prompt('write', { path, content: '# actual tool\n' }), id)
      assert.equal(await readFile(join(dir, 'created.md'), 'utf8'), '# actual tool\n')
      const list = await json(changes(id))
      assert.equal(list.files[0].status, 'created')
      assert.equal(list.files[0].snapshot, false)
      assert.equal(list.files[0].added, 2)
      const diff = await json(`${changes(id)}/diff?path=${encodeURIComponent(path)}`)
      assert.ok(diff.diff.includes('new file mode 100644\n--- /dev/null\n'))
      assert.deepEqual(
        await json(`${changes(id)}/diff?path=${encodeURIComponent(`${prefix}/untracked.txt`)}`),
        { diff: '', diffTruncated: false, source: 'snapshot', found: false },
      )
      await error(
        `${changes(id)}/revert`,
        'POST',
        { path: '../outside-fixture' },
        400,
        'bad_request',
      )
      assert.equal((await json(`${changes(id)}/revert`, 'POST', {})).reverted, 1)
      await absent(join(dir, 'created.md'))
      assert.deepEqual(await json(summary(id)), zeroSummary)
      return { sessionId: id, createdFileDeleted: true }
    }),
  )

  await check('An active real model stream rejects file-change revert with HTTP 409', () =>
    withSession('snapshot-busy-fixture', async (id, dir, prefix) => {
      const path = `${prefix}/busy.txt`
      await writeFile(join(dir, 'busy.txt'), 'original\n')
      await chat(prompt('write', { path, content: 'changed\n' }), id)
      const label = 'snapshot-busy'
      const held = response('/api/chat', 'POST', {
        sessionId: id,
        message: `native-parity-hold:${label}`,
        goalMode: false,
        teamMode: false,
      }).then((result) => {
        assert.equal(result.status, 200)
        return result.text()
      })
      held.catch(() => {})
      try {
        await waitForHeldModel(label)
        await error(`${changes(id)}/revert`, 'POST', { path }, 409, 'session_busy')
        assert.equal(await readFile(join(dir, 'busy.txt'), 'utf8'), 'changed\n')
      } finally {
        try {
          await json(`${base(id)}/abort`, 'POST', {}, 5000)
        } finally {
          heldModelRequests.get(label)?.()
          await held
          await waitForHeldModel(label, false)
        }
      }
      assert.equal((await json(`${changes(id)}/revert`, 'POST', { path })).reverted, 1)
      assert.equal(await readFile(join(dir, 'busy.txt'), 'utf8'), 'original\n')
      return { sessionId: id, status: 409, actualModelConnectionClosed: true }
    }),
  )

  await check('Actual shell execution marks coverage partial before reporting any zero', () =>
    withSession('snapshot-shell-fixture', async (id) => {
      await chat(prompt('bash', { command: 'echo PISPER_SNAPSHOT_PARTIAL' }), id)
      const value = await json(summary(id))
      assert.deepEqual(value, {
        status: 'partial',
        changedFiles: null,
        pendingFiles: null,
        added: null,
        removed: null,
        unknownFiles: 0,
        capped: false,
      })
      const index = JSON.parse(await readFile(join(snapshotDir(id), 'index.json'), 'utf8'))
      assert.equal(index.coverage, 'partial')
      return { sessionId: id, actualTool: 'bash', persistentCoverage: 'partial' }
    }),
  )

  await check('A historical journal without its snapshot index stays unavailable', () =>
    withSession('snapshot-history-fixture', async (id) => {
      const indexPath = join(snapshotDir(id), 'index.json')
      assert.equal(dirname(indexPath), snapshotDir(id))
      // 自己刚建的合成会话移除空 marker，模拟旧版本/淘汰后的历史 journal。
      await rm(indexPath)
      assert.deepEqual(await json(summary(id)), {
        status: 'unavailable',
        changedFiles: null,
        pendingFiles: null,
        added: null,
        removed: null,
        unknownFiles: 0,
        capped: false,
      })
      await json(`${base(id)}/live`)
      assert.equal((await json(summary(id))).status, 'unavailable')
      await absent(indexPath)
      return { sessionId: id, historyIndexNeverFabricated: true }
    }),
  )

  await check('Child tool snapshots belong to the parent and deletion cannot revive them', () =>
    withSession('snapshot-parent-fixture', async (id, dir, prefix) => {
      const path = `${prefix}/child.txt`
      await writeFile(join(dir, 'child.txt'), 'parent baseline\n')
      const child = (
        await json(`${base(id)}/agents`, 'POST', {
          taskName: 'snapshot-child-write',
          message: prompt('write', { path, content: 'child actual write\n' }),
        })
      ).agent
      assert.ok(child.id)
      const completed = await poll(
        async () =>
          (await json(`${base(id)}/agents`)).agents.find((value) => value.id === child.id),
        (value) => value && ['completed', 'failed', 'error', 'interrupted'].includes(value.status),
        'Actual child write',
      )
      assert.equal(completed.status, 'completed', JSON.stringify(completed))
      assert.equal(await readFile(join(dir, 'child.txt'), 'utf8'), 'child actual write\n')
      assert.equal((await json(changes(id))).files[0].path, path)
      assert.equal((await json(summary(id))).pendingFiles, 1)
      const label = 'snapshot-child-delete'
      await json(`${base(id)}/agents`, 'POST', {
        taskName: 'snapshot-child-held',
        message: `native-parity-hold:${label}`,
      })
      try {
        await waitForHeldModel(label)
        const removed = await response(base(id), 'DELETE')
        assert.equal(removed.status, 200, 'Deleting the parent must close its owned child itself')
        assert.equal((await removed.json()).deleted, true)
        await waitForHeldModel(label, false)
        await absent(snapshotDir(id))
        await delay(150)
        await absent(snapshotDir(id))
        await error(summary(id), 'GET', undefined, 404, 'session_not_found')
      } finally {
        heldModelRequests.get(label)?.()
      }
      return { parentSessionId: id, childId: child.id, parentIndexDeletedPermanently: true }
    }),
  )

  await check(
    'Original BOM/newline bytes and first snapshots are staged for actual restart',
    async () => {
      let owned
      let dir
      let keep = false
      try {
        dir = await folder()
        owned = await session('snapshot-restart-fixture')
        const prefix = relative(cwd, dir).replaceAll('\\', '/')
        const path = `${prefix}/restart.txt`
        const original = '\ufeffbefore\r\noriginal\r\n'
        const current = 'actual persisted change\n'
        await writeFile(join(dir, 'restart.txt'), original)
        await chat(prompt('write', { path, content: current }), owned.id)
        const index = JSON.parse(await readFile(join(snapshotDir(owned.id), 'index.json'), 'utf8'))
        assert.equal(index.version, 2)
        assert.equal(index.coverage, 'complete')
        assert.equal(index.entries.length, 1)
        assert.equal(index.entries[0].snapshot, true)
        assert.equal(
          await readFile(join(snapshotDir(owned.id), index.entries[0].key + '.before'), 'utf8'),
          original,
        )
        retained = { id: owned.id, dir, path, original, current }
        keep = true
        return {
          sessionId: owned.id,
          originalBytes: Buffer.byteLength(original),
          snapshotVersion: 2,
        }
      } finally {
        if (!keep) {
          if (owned) await removeSession(owned.id)
          if (dir) await removeFolder(dir)
        }
      }
    },
  )

  if (!retained) return undefined
  return async () => {
    const { id, dir, path, original, current } = retained
    try {
      assert.equal(await readFile(join(dir, 'restart.txt'), 'utf8'), current)
      const list = await json(changes(id))
      assert.equal(list.files.length, 1)
      assert.equal(list.files[0].path, path)
      assert.equal(list.files[0].snapshot, true)
      assert.equal(list.files[0].pending, true)
      assert.equal((await json(summary(id))).status, 'known')
      assert.equal((await json(`${changes(id)}/revert`, 'POST', { path })).reverted, 1)
      assert.equal(await readFile(join(dir, 'restart.txt'), 'utf8'), original)
      assert.deepEqual(await json(summary(id)), zeroSummary)
      return { sessionId: id, actualRestartRevert: true, bomAndNewlineBytesPreserved: true }
    } finally {
      try {
        await removeSession(id)
      } finally {
        await removeFolder(dir)
      }
    }
  }
}
