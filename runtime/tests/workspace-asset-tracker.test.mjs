import assert from 'node:assert/strict'
import { getEventListeners } from 'node:events'
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setImmediate as nextTick } from 'node:timers/promises'
import test from 'node:test'
import { WorkspaceAssetTracker } from '../services/workspace-asset-tracker.mjs'
import { createToolGatewayTool } from '../tools/app/tool-gateway.mjs'

function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

async function waitingForLock(signal) {
  const deadline = Date.now() + 3_000
  while (!getEventListeners(signal, 'abort').length) {
    assert.ok(Date.now() < deadline, '调用应进入可取消的锁等待')
    await nextTick()
  }
}

async function fixture(t, archiveHook) {
  const root = await mkdtemp(join(tmpdir(), 'pisper-asset-tracker-'))
  const cwd = join(root, 'workspace')
  const dataDir = join(root, 'data')
  await Promise.all([mkdir(cwd), mkdir(dataDir)])
  t.after(() => rm(root, { recursive: true, force: true }))
  const archived = []
  const attempts = []
  const warnings = []
  const archive = async (sessionId, path) => {
    const content = await readFile(path, 'utf8')
    const asset = { id: `${sessionId}:${path}`, sessionId, path, content }
    attempts.push(asset)
    await archiveHook?.(asset, attempts.length)
    archived.push(asset)
    return asset
  }
  const createTracker = (trackerDataDir = dataDir) =>
    new WorkspaceAssetTracker({
      dataDir: trackerDataDir,
      archive,
      warn: (error) => warnings.push(error),
    })
  const tracker = createTracker()
  const run = (sessionId, execute, options = {}) =>
    tracker.run({ sessionId, cwd, name: 'bash', args: { command: 'fixture' }, ...options }, execute)
  return { cwd, dataDir, tracker, createTracker, run, archived, attempts, warnings }
}

async function directoryAlias(t, target, path) {
  try {
    // Windows 的目录 junction 不依赖管理员权限，其他平台使用目录符号链接。
    await symlink(target, path, process.platform === 'win32' ? 'junction' : 'dir')
    return true
  } catch (error) {
    if (!['EPERM', 'EACCES', 'ENOSYS', 'ENOTSUP', 'EOPNOTSUPP'].includes(error.code)) throw error
    t.skip(`当前环境无法创建目录链接：${error.code} ${error.message}`)
    return false
  }
}

function dispatch(session, tool, args, signal) {
  return (async () => {
    const decision = await session.agent.beforeToolCall?.(
      { toolCall: { id: 'fixture-call', name: tool.name }, args, context: { tools: [tool] } },
      signal,
    )
    if (decision?.block || signal?.aborted) return decision
    return tool.execute('fixture-call', args, signal)
  })()
}

test('concurrent shell calls in the same cwd archive only their own session files', async (t) => {
  const f = await fixture(t)
  const started = deferred()
  const release = deferred()
  const controller = new AbortController()
  const first = f.run('session-a', async () => {
    await writeFile(join(f.cwd, 'a.txt'), 'from a')
    started.resolve()
    await release.promise
    return 'a-result'
  })
  await started.promise
  let secondExecuted = false
  const second = f.run(
    'session-b',
    async () => {
      secondExecuted = true
      await writeFile(join(f.cwd, 'b.txt'), 'from b')
      return 'b-result'
    },
    { signal: controller.signal },
  )
  try {
    await waitingForLock(controller.signal)
    assert.equal(secondExecuted, false)
  } finally {
    release.resolve()
  }
  assert.deepEqual(await Promise.all([first, second]), ['a-result', 'b-result'])
  const a = await f.tracker.drain('session-a')
  const b = await f.tracker.drain('session-b')
  assert.deepEqual(
    a.map(({ path, content }) => [path, content]),
    [[join(f.cwd, 'a.txt'), 'from a']],
  )
  assert.deepEqual(
    b.map(({ path, content }) => [path, content]),
    [[join(f.cwd, 'b.txt'), 'from b']],
  )
  assert.equal(f.attempts.length, 2)
  assert.deepEqual(await f.tracker.drain('session-a'), [])
})

test('shell overwrites archive the updated content at the same path', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, 'report.txt')
  await f.run('session-a', () => writeFile(path, 'first'))
  assert.deepEqual(
    (await f.tracker.drain('session-a')).map((asset) => asset.content),
    ['first'],
  )
  await f.run('session-a', () => writeFile(path, 'second version'))
  assert.deepEqual(
    (await f.tracker.drain('session-a')).map((asset) => asset.content),
    ['second version'],
  )
  assert.deepEqual(
    f.archived.map((asset) => asset.path),
    [path, path],
  )
})

test('empty turns and read tools do not claim external files', async (t) => {
  const f = await fixture(t)
  f.tracker.install({ agent: {} }, { sessionId: 'empty', cwd: f.cwd })
  await writeFile(join(f.cwd, 'outside-turn.txt'), 'external')
  assert.deepEqual(await f.tracker.drain('empty'), [])
  await f.run(
    'reader',
    async () => {
      // 模拟只读工具执行期间由工作区外部写入的文件。
      await writeFile(join(f.cwd, 'outside-read.txt'), 'external during read')
      return readFile(join(f.cwd, 'outside-turn.txt'), 'utf8')
    },
    { name: 'read', args: { path: 'outside-turn.txt' } },
  )
  assert.deepEqual(await f.tracker.drain('reader'), [])
  await f.run('readonly-shell', () => readFile(join(f.cwd, 'outside-turn.txt'), 'utf8'))
  assert.deepEqual(await f.tracker.drain('readonly-shell'), [])
  assert.deepEqual(f.attempts, [])
})

test('one shell call archives all 201 generated files', async (t) => {
  const f = await fixture(t)
  const expected = Array.from({ length: 201 }, (_, index) => [
    join(f.cwd, `report-${String(index).padStart(3, '0')}.txt`),
    `report ${index}`,
  ])
  await f.run('batch', () =>
    Promise.all(expected.map(([path, content]) => writeFile(path, content))),
  )
  const assets = await f.tracker.drain('batch')
  assert.equal(assets.length, 201)
  assert.deepEqual(
    assets.map(({ path, content }) => [path, content]).sort(([a], [b]) => a.localeCompare(b)),
    expected,
  )
  assert.equal(f.attempts.length, 201)
  assert.deepEqual(f.warnings, [])
})

test('drain retries a failed archive once and consumes successful deliveries', async (t) => {
  const failure = new Error('archive unavailable')
  const f = await fixture(t, (_asset, attempt) => {
    if (attempt === 1) throw failure
  })
  await f.run('retry', () => writeFile(join(f.cwd, 'retry.txt'), 'retry content'))
  assert.deepEqual(f.archived, [])
  assert.deepEqual(f.warnings, [failure])
  const assets = await f.tracker.drain('retry')
  assert.equal(assets.length, 1)
  assert.equal(assets[0].content, 'retry content')
  assert.equal(f.attempts.length, 2)
  assert.deepEqual(await f.tracker.drain('retry'), [])
  assert.equal(f.attempts.length, 2)
})

test('a new tracker restores failed archives from dataDir and persists their completion', async (t) => {
  const f = await fixture(t, (_asset, attempt) => {
    if (attempt === 1) throw new Error('temporary archive failure')
  })
  const path = join(f.cwd, 'persisted.txt')
  await f.run('restored', () => writeFile(path, 'persisted content'))
  const restored = f.createTracker()
  assert.deepEqual(await restored.drain('other-session'), [])
  assert.equal(f.attempts.length, 1)
  assert.deepEqual(
    (await restored.drain('restored')).map(({ sessionId, path: assetPath, content }) => [
      sessionId,
      assetPath,
      content,
    ]),
    [['restored', path, 'persisted content']],
  )
  assert.deepEqual(await f.createTracker().drain('restored'), [])
  assert.equal(f.attempts.length, 2)
})

test('retry never attributes a later session overwrite to the original session', async (t) => {
  const f = await fixture(t, (asset) => {
    if (asset.sessionId === 'old') throw new Error('old session archive failed')
  })
  const path = join(f.cwd, 'shared.txt')
  await f.run('old', () => writeFile(path, 'old content'))
  await f.run('new', () => writeFile(path, 'new content is longer'))
  assert.deepEqual(await f.tracker.drain('old'), [])
  assert.deepEqual(
    (await f.tracker.drain('new')).map((asset) => asset.content),
    ['new content is longer'],
  )
  assert.deepEqual(
    f.attempts.map(({ sessionId, content }) => [sessionId, content]),
    [
      ['old', 'old content'],
      ['new', 'new content is longer'],
    ],
  )
  assert.deepEqual(await f.createTracker().drain('old'), [])
  assert.equal(f.attempts.length, 2)
})

test('restored retries discard externally replaced files without claiming their new content', async (t) => {
  const f = await fixture(t, (_asset, attempt) => {
    if (attempt === 1) throw new Error('archive failed before restart')
  })
  const path = join(f.cwd, 'external-overwrite.txt')
  await f.run('old', () => writeFile(path, 'old'))
  await writeFile(path, 'external replacement')
  assert.deepEqual(await f.createTracker().drain('old'), [])
  assert.equal(f.attempts.length, 1)
  assert.deepEqual(f.archived, [])
  assert.deepEqual(await f.createTracker().drain('old'), [])
})

test('explicit write and edit capture their target including paths outside cwd', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, '..', 'explicit.txt')
  await f.run('writer', () => writeFile(path, 'written'), {
    name: 'write',
    args: { path, content: 'written' },
  })
  await f.run('editor', () => writeFile(path, 'edited content'), {
    name: 'edit',
    args: { path, edits: [{ oldText: 'written', newText: 'edited content' }] },
  })
  assert.deepEqual(
    (await f.tracker.drain('writer')).map((asset) => asset.content),
    ['written'],
  )
  assert.deepEqual(
    (await f.tracker.drain('editor')).map((asset) => asset.content),
    ['edited content'],
  )
  assert.deepEqual(
    f.archived.map((asset) => asset.path),
    [path, path],
  )
})

test('an explicit no-op edit does not claim unrelated external files', async (t) => {
  const f = await fixture(t)
  const path = join(f.cwd, 'existing.txt')
  await writeFile(path, 'unchanged')
  const result = await f.run(
    'editor',
    async () => {
      await writeFile(join(f.cwd, 'external.txt'), 'external')
      return 'no matching edit'
    },
    { name: 'edit', args: { path: 'existing.txt', edits: [] } },
  )
  assert.equal(result, 'no matching edit')
  assert.deepEqual(await f.tracker.drain('editor'), [])
  assert.deepEqual(f.attempts, [])
})

test('install wraps call_tool routed to bash while preserving execution arguments and result', async (t) => {
  const f = await fixture(t)
  const signal = new AbortController().signal
  const args = { name: 'bash', arguments: { command: 'generate report' } }
  const result = { content: [{ type: 'text', text: 'written' }] }
  let approvals = 0
  let executions = 0
  const session = {
    agent: {
      beforeToolCall: async () => {
        approvals += 1
      },
    },
  }
  const tool = {
    name: 'call_tool',
    async execute(id, actualArgs, actualSignal) {
      executions += 1
      assert.equal(this, tool)
      assert.equal(id, 'fixture-call')
      assert.equal(actualArgs, args)
      assert.equal(actualSignal, signal)
      await writeFile(join(f.cwd, 'routed.txt'), 'routed bash content')
      return result
    },
  }
  f.tracker.install(session, { sessionId: 'routed', cwd: f.cwd })
  f.tracker.install(session, { sessionId: 'routed', cwd: f.cwd })
  assert.equal(await dispatch(session, tool, args, signal), result)
  assert.equal(approvals, 1)
  assert.equal(executions, 1)
  assert.deepEqual(
    (await f.tracker.drain('routed')).map((asset) => asset.content),
    ['routed bash content'],
  )
  assert.equal(f.attempts.length, 1)
})

test('blocked calls neither execute nor capture external files, even after an allowed call', async (t) => {
  const f = await fixture(t)
  const denied = { block: true, reason: 'not approved' }
  let decision = denied
  let executions = 0
  const session = { agent: { beforeToolCall: async () => decision } }
  const tool = {
    name: 'bash',
    async execute() {
      executions += 1
      await writeFile(join(f.cwd, 'allowed.txt'), 'approved content')
    },
  }
  const original = tool.execute
  f.tracker.install(session, { sessionId: 'approval', cwd: f.cwd })
  await writeFile(join(f.cwd, 'external.txt'), 'external')
  assert.equal(await dispatch(session, tool, { command: 'denied' }), denied)
  assert.equal(tool.execute, original)
  assert.equal(executions, 0)
  assert.deepEqual(await f.tracker.drain('approval'), [])
  decision = undefined
  await dispatch(session, tool, { command: 'allowed' })
  assert.equal((await f.tracker.drain('approval')).length, 1)
  decision = denied
  await writeFile(join(f.cwd, 'external-later.txt'), 'external later')
  assert.equal(await dispatch(session, tool, { command: 'denied again' }), denied)
  assert.equal(executions, 1)
  assert.deepEqual(await f.tracker.drain('approval'), [])
  assert.equal(f.attempts.length, 1)
})

test('aborting a lock waiter prevents execution and allows later calls to acquire the lock', async (t) => {
  const f = await fixture(t)
  const started = deferred()
  const release = deferred()
  const first = f.run('holder', async () => {
    started.resolve()
    await release.promise
    await writeFile(join(f.cwd, 'holder.txt'), 'holder')
  })
  await started.promise
  const controller = new AbortController()
  const reason = new Error('cancelled while waiting')
  let executed = false
  const waiting = f.run(
    'cancelled',
    async () => {
      executed = true
    },
    { signal: controller.signal },
  )
  const rejected = assert.rejects(waiting, (error) => error === reason)
  try {
    await waitingForLock(controller.signal)
    controller.abort(reason)
    await rejected
    assert.equal(executed, false)
    assert.equal(getEventListeners(controller.signal, 'abort').length, 0)
  } finally {
    release.resolve()
  }
  await first
  await f.run('following', () => writeFile(join(f.cwd, 'following.txt'), 'following'))
  assert.deepEqual(await f.tracker.drain('cancelled'), [])
  assert.deepEqual(
    (await f.tracker.drain('following')).map((asset) => asset.content),
    ['following'],
  )
  assert.deepEqual(
    f.archived.map((asset) => asset.sessionId),
    ['holder', 'following'],
  )
})

test('an already aborted file call never executes or captures files', async (t) => {
  const f = await fixture(t)
  const controller = new AbortController()
  const reason = new Error('cancelled before execution')
  controller.abort(reason)
  let executed = false
  await assert.rejects(
    f.run(
      'cancelled',
      async () => {
        executed = true
      },
      { signal: controller.signal },
    ),
    (error) => error === reason,
  )
  assert.equal(executed, false)
  assert.deepEqual(await f.tracker.drain('cancelled'), [])
  assert.deepEqual(f.attempts, [])
})

test('gateway names with whitespace still acquire the workspace lock and capture their output', async (t) => {
  const f = await fixture(t)
  const started = deferred()
  const release = deferred()
  const controller = new AbortController()
  let executions = 0
  const authorizedNames = []
  const gateway = createToolGatewayTool({
    getTool: (name) =>
      name === 'bash'
        ? {
            parameters: { type: 'object' },
            async execute() {
              executions += 1
              await writeFile(join(f.cwd, 'gateway.txt'), 'gateway output')
              return { content: [{ type: 'text', text: 'gateway result' }] }
            },
          }
        : null,
    authorize: async ({ toolName }) => {
      authorizedNames.push(toolName)
    },
  })
  const session = { agent: {} }
  f.tracker.install(session, { sessionId: 'gateway', cwd: f.cwd })
  const holder = f.run('holder', async () => {
    await writeFile(join(f.cwd, 'holder.txt'), 'holder output')
    started.resolve()
    await release.promise
  })
  await started.promise
  const routed = dispatch(
    session,
    gateway,
    { name: ' \tbash\n ', arguments: { command: 'generate report' } },
    controller.signal,
  )
  try {
    await waitingForLock(controller.signal)
    assert.equal(executions, 0)
  } finally {
    release.resolve()
    await Promise.allSettled([holder, routed])
  }
  assert.equal((await routed).details.gatewayToolName, 'bash')
  assert.equal(executions, 1)
  assert.deepEqual(authorizedNames, ['bash'])
  assert.deepEqual(
    (await f.tracker.drain('holder')).map((asset) => asset.content),
    ['holder output'],
  )
  assert.deepEqual(
    (await f.tracker.drain('gateway')).map((asset) => asset.content),
    ['gateway output'],
  )
  assert.equal(f.attempts.length, 2)
})

test('aborting while ready is pending prevents execution and releases the acquired lock', async (t) => {
  const f = await fixture(t)
  const originalReady = f.tracker.ready
  await originalReady
  const enteredReady = deferred()
  const releaseReady = deferred()
  const controller = new AbortController()
  const reason = new Error('cancelled during capture preparation')
  // 等待真正进入 ready 后才取消，避免把此回归误测成取得锁之前的取消。
  f.tracker.ready = {
    then(onFulfilled, onRejected) {
      enteredReady.resolve()
      return releaseReady.promise.then(onFulfilled, onRejected)
    },
  }
  let executions = 0
  const cancelledPath = join(f.cwd, 'cancelled.txt')
  const running = f.run(
    'cancelled',
    async () => {
      executions += 1
      await writeFile(cancelledPath, 'must not execute')
    },
    { signal: controller.signal },
  )
  const rejected = assert.rejects(running, (error) => error === reason)
  try {
    await enteredReady.promise
    controller.abort(reason)
    releaseReady.resolve()
    await rejected
  } finally {
    releaseReady.resolve()
    await Promise.allSettled([running, rejected])
    f.tracker.ready = originalReady
  }
  assert.equal(executions, 0)
  await assert.rejects(readFile(cancelledPath), { code: 'ENOENT' })
  assert.deepEqual(await f.tracker.drain('cancelled'), [])
  await f.run('following', () => writeFile(join(f.cwd, 'following.txt'), 'following output'))
  assert.deepEqual(
    (await f.tracker.drain('following')).map((asset) => asset.content),
    ['following output'],
  )
  assert.deepEqual(
    f.archived.map((asset) => asset.sessionId),
    ['following'],
  )
})

test('a cwd directory alias pointing at dataDir does not capture runtime files', async (t) => {
  const f = await fixture(t)
  const alias = join(f.cwd, '..', 'runtime-cwd-alias')
  if (!(await directoryAlias(t, f.dataDir, alias))) return
  await f.run(
    'internal-shell',
    () => writeFile(join(alias, 'runtime-state.json'), '{"internal":true}'),
    { cwd: alias },
  )
  assert.equal(await readFile(join(f.dataDir, 'runtime-state.json'), 'utf8'), '{"internal":true}')
  assert.deepEqual(await f.tracker.drain('internal-shell'), [])
  assert.deepEqual(f.attempts, [])
  assert.deepEqual(f.warnings, [])
})

test('explicit writes through an intermediate alias exclude dataDir configured by another alias', async (t) => {
  const f = await fixture(t)
  const configuredAlias = join(f.cwd, '..', 'configured-data-alias')
  const outputAlias = join(f.cwd, 'output-alias')
  if (!(await directoryAlias(t, f.dataDir, configuredAlias))) return
  if (!(await directoryAlias(t, f.dataDir, outputAlias))) return
  await mkdir(join(f.dataDir, 'nested'))
  const tracker = f.createTracker(configuredAlias)
  const path = join(outputAlias, 'nested', 'runtime-state.json')
  await tracker.run(
    { sessionId: 'internal-write', cwd: f.cwd, name: 'write', args: { path } },
    () => writeFile(path, '{"version":1}'),
  )
  await tracker.run({ sessionId: 'internal-edit', cwd: f.cwd, name: 'edit', args: { path } }, () =>
    writeFile(path, '{"version":2,"updated":true}'),
  )
  assert.equal(
    await readFile(join(f.dataDir, 'nested', 'runtime-state.json'), 'utf8'),
    '{"version":2,"updated":true}',
  )
  assert.deepEqual(await tracker.drain('internal-write'), [])
  assert.deepEqual(await tracker.drain('internal-edit'), [])
  assert.deepEqual(f.attempts, [])
  assert.deepEqual(f.warnings, [])
})

test('a failed shell still archives its output and releases the workspace lock', async (t) => {
  const f = await fixture(t)
  const failure = new Error('shell exited with status 1')
  await assert.rejects(
    f.run('failed-shell', async () => {
      await writeFile(join(f.cwd, 'partial.txt'), 'partial output')
      throw failure
    }),
    (error) => error === failure,
  )
  assert.deepEqual(
    (await f.tracker.drain('failed-shell')).map((asset) => asset.content),
    ['partial output'],
  )
  await f.run('next-shell', () => writeFile(join(f.cwd, 'next.txt'), 'next output'))
  assert.deepEqual(
    (await f.tracker.drain('next-shell')).map((asset) => asset.content),
    ['next output'],
  )
})
