// 会话文件变更服务测试：无 VCS 环境下的快照、diff、撤销与审批。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat, utimes, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import { SessionFileChangesService } from '../services/session-file-changes.mjs'

async function fixture(t) {
  const cwd = await mkdtemp(join(tmpdir(), 'pisper-file-changes-'))
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-file-changes-data-'))
  t.after(async () => {
    await rm(cwd, { recursive: true, force: true })
    await rm(dataDir, { recursive: true, force: true })
  })
  const service = new SessionFileChangesService({ dataDir, warn: () => {} })
  return { cwd, dataDir, service }
}

// 模拟一次 write 工具调用：安装钩子后执行 beforeToolCall + tool.execute。
async function runWriteTool(f, sessionId, path, content) {
  await f.service.markSessionTracked(sessionId, f.cwd)
  const tool = {
    name: 'write',
    execute: async () => {
      await writeFile(join(f.cwd, path), content, 'utf8')
      return { ok: true }
    },
  }
  const session = { agent: {}, sessionId }
  f.service.install(session, { sessionId, cwd: f.cwd })
  const decision = await session.agent.beforeToolCall(
    { toolCall: { name: 'write' }, args: { path }, context: { tools: [tool] } },
    undefined,
  )
  assert.equal(decision, undefined)
  await tool.execute('call-1', { path }, undefined)
}

test('快照、清单统计与 diff：编辑既有文件', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'a.txt'), 'one\ntwo\nthree\n', 'utf8')
  await runWriteTool(f, 's1', 'a.txt', 'one\nTWO\nthree\nfour\n')
  const list = await f.service.list('s1', f.cwd)
  assert.equal(list.summary.files, 1)
  assert.equal(list.files[0].path, 'a.txt')
  assert.equal(list.files[0].status, 'modified')
  assert.equal(list.files[0].added, 2)
  assert.equal(list.files[0].removed, 1)
  assert.equal(list.files[0].pending, true)
  const diff = await f.service.diff('s1', f.cwd, 'a.txt')
  assert.match(diff.diff, /--- a\/a\.txt/)
  assert.match(diff.diff, /\+TWO/)
  assert.match(diff.diff, /\+four/)
})

test('新建文件：diff 标记 new file，撤销即删除', async (t) => {
  const f = await fixture(t)
  await runWriteTool(f, 's2', 'new.md', '# hello\n')
  let list = await f.service.list('s2', f.cwd)
  assert.equal(list.files[0].status, 'created')
  assert.equal(list.files[0].canRevert, true)
  assert.equal(list.files[0].added, 2)
  const diff = await f.service.diff('s2', f.cwd, 'new.md')
  assert.match(diff.diff, /--- \/dev\/null/)
  const result = await f.service.revert('s2', f.cwd, 'new.md')
  assert.equal(result.reverted, 1)
  await assert.rejects(stat(join(f.cwd, 'new.md')))
  list = result.files
  assert.equal(list.files[0].reverted, true)
  assert.equal(list.files[0].pending, false)
})

test('撤销恢复到最初内容：同一文件多次编辑只保留首次快照', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'b.txt'), 'origin\n', 'utf8')
  await runWriteTool(f, 's3', 'b.txt', 'first\n')
  await runWriteTool(f, 's3', 'b.txt', 'second\n')
  assert.equal(await readFile(join(f.cwd, 'b.txt'), 'utf8'), 'second\n')
  await f.service.revert('s3', f.cwd, 'b.txt')
  assert.equal(await readFile(join(f.cwd, 'b.txt'), 'utf8'), 'origin\n')
})

test('审批清除待办；再次编辑会重新置为未审批', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'c.txt'), 'x\n', 'utf8')
  await runWriteTool(f, 's4', 'c.txt', 'y\n')
  let list = await f.service.list('s4', f.cwd)
  assert.equal(list.summary.pending, 1)
  list = (await f.service.approve('s4', f.cwd)).files
  assert.equal(list.summary.pending, 0)
  assert.equal(list.files[0].approved, true)
  await runWriteTool(f, 's4', 'c.txt', 'z\n')
  list = await f.service.list('s4', f.cwd)
  assert.equal(list.files[0].approved, false)
  assert.equal(list.summary.pending, 1)
})

test('无快照能力的文件只记录变动事实，不可撤销', async (t) => {
  const f = await fixture(t)
  const binary = join(f.cwd, 'blob.bin')
  await writeFile(binary, Buffer.from([0x00, 0x01, 0x02]))
  const tool = {
    name: 'write',
    execute: async () => {
      await writeFile(binary, Buffer.from([0x00, 0x03]))
      return { ok: true }
    },
  }
  const session = { agent: {}, sessionId: 's5' }
  f.service.install(session, { sessionId: 's5', cwd: f.cwd })
  await session.agent.beforeToolCall(
    { toolCall: { name: 'write' }, args: { path: 'blob.bin' }, context: { tools: [tool] } },
    undefined,
  )
  await tool.execute('call-2', { path: 'blob.bin' }, undefined)
  const list = await f.service.list('s5', f.cwd)
  assert.equal(list.files[0].snapshot, false)
  assert.equal(list.files[0].status, 'modified')
  assert.equal(list.files[0].canRevert, false)
  assert.equal(list.files[0].pending, true)
  const result = await f.service.revert('s5', f.cwd, 'blob.bin')
  assert.equal(result.reverted, 0)
  assert.deepEqual([...(await readFile(binary))], [0x00, 0x03])
})

test('call_tool 透传调用同样被追踪', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'd.txt'), 'before\n', 'utf8')
  const tool = {
    name: 'call_tool',
    execute: async () => {
      await writeFile(join(f.cwd, 'd.txt'), 'after\n', 'utf8')
      return { ok: true }
    },
  }
  const session = { agent: {}, sessionId: 's6' }
  f.service.install(session, { sessionId: 's6', cwd: f.cwd })
  await session.agent.beforeToolCall(
    {
      toolCall: { name: 'call_tool' },
      args: { name: 'write', arguments: { path: 'd.txt' } },
      context: { tools: [tool] },
    },
    undefined,
  )
  await tool.execute('call-3', { name: 'write', arguments: { path: 'd.txt' } }, undefined)
  const list = await f.service.list('s6', f.cwd)
  assert.equal(list.summary.files, 1)
  await f.service.revert('s6', f.cwd)
  assert.equal(await readFile(join(f.cwd, 'd.txt'), 'utf8'), 'before\n')
})

test('工作区外的路径不追踪', async (t) => {
  const f = await fixture(t)
  const outside = join(f.dataDir, 'outside.txt')
  await writeFile(outside, 'x\n', 'utf8')
  const tool = {
    name: 'write',
    execute: async () => {
      await writeFile(outside, 'y\n', 'utf8')
      return { ok: true }
    },
  }
  const session = { agent: {}, sessionId: 's7' }
  f.service.install(session, { sessionId: 's7', cwd: f.cwd })
  await session.agent.beforeToolCall(
    { toolCall: { name: 'write' }, args: { path: outside }, context: { tools: [tool] } },
    undefined,
  )
  await tool.execute('call-4', { path: outside }, undefined)
  const list = await f.service.list('s7', f.cwd)
  assert.equal(list.summary.files, 0)
})

test('同一会话的并行写入串行化，始终撤销到首次修改前', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'parallel.txt'), 'before\n', 'utf8')
  let releaseFirst
  const firstGate = new Promise((resolve) => {
    releaseFirst = resolve
  })
  let firstStarted
  const firstStartedPromise = new Promise((resolve) => {
    firstStarted = resolve
  })
  const first = f.service.run(
    { sessionId: 'parallel', cwd: f.cwd, name: 'write', args: { path: 'parallel.txt' } },
    async () => {
      firstStarted()
      await firstGate
      await writeFile(join(f.cwd, 'parallel.txt'), 'first\n', 'utf8')
    },
  )
  await firstStartedPromise
  let secondStarted = false
  const second = f.service.run(
    { sessionId: 'parallel', cwd: f.cwd, name: 'write', args: { path: 'parallel.txt' } },
    async () => {
      secondStarted = true
      await writeFile(join(f.cwd, 'parallel.txt'), 'second\n', 'utf8')
    },
  )
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(secondStarted, false)
  releaseFirst()
  await Promise.all([first, second])
  await f.service.revert('parallel', f.cwd, 'parallel.txt')
  assert.equal(await readFile(join(f.cwd, 'parallel.txt'), 'utf8'), 'before\n')
})

test('facade 向 HTTP 返回平铺的文件变更清单', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'facade.txt'), 'before\n', 'utf8')
  await runWriteTool(f, 'facade', 'facade.txt', 'after\n')
  const runtime = Object.assign(Object.create(AgentRuntimeFacade.prototype), {
    fileChanges: f.service,
    sessionWorkspaceCwd: async () => f.cwd,
    sessionRunIsActive: () => false,
  })
  const approved = await runtime.approveSessionFileChanges('facade', 'facade.txt')
  assert.equal(approved.summary.files, 1)
  assert.equal(approved.files[0].approved, true)
  const reverted = await runtime.revertSessionFileChanges('facade', 'facade.txt')
  assert.equal(reverted.reverted, 1)
  assert.equal(reverted.summary.pending, 0)
})

test('服务重启后快照与索引仍可恢复（持久化）', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'e.txt'), 'keep\n', 'utf8')
  await runWriteTool(f, 's8', 'e.txt', 'changed\n')
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  const list = await revived.list('s8', f.cwd)
  assert.equal(list.files[0].added, 1)
  await revived.revert('s8', f.cwd, 'e.txt')
  assert.equal(await readFile(join(f.cwd, 'e.txt'), 'utf8'), 'keep\n')
})

test('clear 移除会话全部快照', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'f.txt'), 'x\n', 'utf8')
  await runWriteTool(f, 's9', 'f.txt', 'y\n')
  await f.service.clear('s9')
  const list = await f.service.list('s9', f.cwd)
  assert.equal(list.summary.files, 0)
})

test('摘要只在有持久化索引时把零写入判为已知零改动', async (t) => {
  const f = await fixture(t)
  assert.deepEqual(await f.service.summary('never-tracked', f.cwd), {
    status: 'unavailable',
    changedFiles: null,
    pendingFiles: null,
    added: null,
    removed: null,
    unknownFiles: 0,
    capped: false,
  })
  await f.service.markSessionTracked('new-empty', f.cwd)
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  assert.deepEqual(await revived.summary('new-empty', f.cwd), {
    status: 'known',
    changedFiles: 0,
    pendingFiles: 0,
    added: 0,
    removed: 0,
    unknownFiles: 0,
    capped: false,
  })
})

test('摘要按当前净变更计数，审批与恢复不保留历史改动徽标', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'original.txt'), 'before\n', 'utf8')
  await runWriteTool(f, 'summary', 'original.txt', 'after\nextra\n')
  await runWriteTool(f, 'summary', 'new.txt', 'new\n')
  let summary = await f.service.summary('summary', f.cwd)
  assert.equal(summary.status, 'known')
  assert.equal(summary.changedFiles, 2)
  assert.equal(summary.pendingFiles, 2)
  assert(summary.added > 0)
  assert(summary.removed > 0)

  await f.service.approve('summary', f.cwd, 'original.txt')
  summary = await f.service.summary('summary', f.cwd)
  assert.equal(summary.changedFiles, 2)
  assert.equal(summary.pendingFiles, 1)

  await writeFile(join(f.cwd, 'original.txt'), 'before\n', 'utf8')
  await rm(join(f.cwd, 'new.txt'))
  summary = await f.service.summary('summary', f.cwd)
  assert.deepEqual(summary, {
    status: 'known',
    changedFiles: 0,
    pendingFiles: 0,
    added: 0,
    removed: 0,
    unknownFiles: 0,
    capped: false,
  })
  assert.equal((await f.service.list('summary', f.cwd)).summary.files, 2)
})

test('摘要对删除文件计入净变更，但缺失基线与超预算均显式标记未知', async (t) => {
  const f = await fixture(t)
  await writeFile(join(f.cwd, 'deleted.txt'), 'before\n', 'utf8')
  await runWriteTool(f, 'deletion', 'deleted.txt', 'after\n')
  await rm(join(f.cwd, 'deleted.txt'))
  let summary = await f.service.summary('deletion', f.cwd)
  assert.equal(summary.status, 'known')
  assert.equal(summary.changedFiles, 1)
  assert.equal(summary.pendingFiles, 1)
  assert.equal(summary.added, 0)
  assert(summary.removed > 0)

  const [entry] = await f.service.load('deletion')
  await rm(f.service.snapshotPath('deletion', entry.key))
  summary = await f.service.summary('deletion', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.unknownFiles, 1)
  assert.equal(summary.changedFiles, null)
  assert.equal(summary.removed, null)

  await runWriteTool(f, 'large', 'large.txt', 'x'.repeat(600 * 1024))
  summary = await f.service.summary('large', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.unknownFiles, 1)
  assert.equal(summary.added, null)

  await writeFile(join(f.cwd, 'many-lines.txt'), 'start\n', 'utf8')
  await runWriteTool(f, 'many-lines', 'many-lines.txt', 'line\n'.repeat(2_100))
  summary = await f.service.summary('many-lines', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.unknownFiles, 1)

  for (let index = 0; index < 6; index += 1)
    await runWriteTool(f, 'aggregate-budget', `chunk-${index}.txt`, 'x'.repeat(400 * 1024))
  summary = await f.service.summary('aggregate-budget', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.unknownFiles, 1)
})

test('200 条索引上限与旧快照目录淘汰不会伪装成精确数量', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('capped', f.cwd)
  const entries = Array.from({ length: 200 }, (_, index) => ({
    path: `file-${index}.txt`,
    key: '0'.repeat(24),
    beforeExists: false,
    snapshot: false,
    approved: false,
  }))
  const cappedMeta = JSON.parse(await readFile(f.service.indexPath('capped'), 'utf8'))
  await writeFile(f.service.indexPath('capped'), JSON.stringify({ ...cappedMeta, entries }))
  let revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  const capped = await revived.summary('capped', f.cwd)
  assert.equal(capped.status, 'partial')
  assert.equal(capped.capped, true)
  assert.equal(capped.changedFiles, null)

  const ids = Array.from({ length: 51 }, (_, index) => `old-${index}`)
  await Promise.all(
    ids.map(async (id) => {
      await f.service.markSessionTracked(id, f.cwd)
      const meta = JSON.parse(await readFile(f.service.indexPath(id), 'utf8'))
      await writeFile(
        f.service.indexPath(id),
        JSON.stringify({
          ...meta,
          entries: [
            {
              path: 'once-written.txt',
              key: '0'.repeat(24),
              beforeExists: false,
              snapshot: false,
              approved: false,
            },
          ],
        }),
      )
    }),
  )
  await utimes(f.service.sessionDir(ids[0]), new Date(2000, 0, 1), new Date(2000, 0, 1))
  revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  await revived.pruned
  assert.equal((await revived.summary(ids[0], f.cwd)).status, 'unavailable')
  assert.equal((await revived.summary(ids.at(-1), f.cwd)).status, 'known')
})

test('空会话标记不会挤占真实文件快照的 50 个保留名额', async (t) => {
  const f = await fixture(t)
  await runWriteTool(f, 'real-change', 'kept.txt', 'content\n')
  await Promise.all(
    Array.from({ length: 51 }, (_, index) => f.service.markSessionTracked(`empty-${index}`, f.cwd)),
  )
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  assert.equal((await revived.summary('real-change', f.cwd)).status, 'known')
  assert.equal((await revived.summary('empty-0', f.cwd)).status, 'known')
})

test('facade 摘要只委托会话快照，不读取整个工作区的 VCS 改动', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('facade-summary', f.cwd)
  const runtime = Object.assign(Object.create(AgentRuntimeFacade.prototype), {
    fileChanges: f.service,
    sessionWorkspaceCwd: async () => f.cwd,
    vcsChanges: {
      getChanges: () => {
        throw new Error('VCS must not be consulted')
      },
    },
  })
  assert.equal((await runtime.getSessionChangeSummary('facade-summary')).changedFiles, 0)
})

test('摘要拒绝索引中的工作区外路径', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('unsafe-path', f.cwd)
  const meta = JSON.parse(await readFile(f.service.indexPath('unsafe-path'), 'utf8'))
  await writeFile(
    f.service.indexPath('unsafe-path'),
    JSON.stringify({
      ...meta,
      entries: [
        {
          path: '../outside.txt',
          key: '0'.repeat(24),
          beforeExists: false,
          snapshot: false,
          approved: false,
        },
      ],
    }),
  )
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  const summary = await revived.summary('unsafe-path', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.unknownFiles, 1)
})

test('只读工具维持完整覆盖，命令与未知工具在执行前持久降级摘要', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('tool-coverage', f.cwd)
  const session = { agent: {}, sessionId: 'tool-coverage' }
  f.service.install(session, { sessionId: 'tool-coverage', cwd: f.cwd })
  const call = (name, args = {}) =>
    session.agent.beforeToolCall({ toolCall: { name }, args, context: { tools: [] } })

  await call('read', { path: 'example.txt' })
  await call('grep', { pattern: 'example' })
  assert.equal((await f.service.summary('tool-coverage', f.cwd)).status, 'known')

  await call('bash', { command: 'printf something' })
  let summary = await f.service.summary('tool-coverage', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.changedFiles, null)
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  summary = await revived.summary('tool-coverage', f.cwd)
  assert.equal(summary.status, 'partial')
  assert.equal(summary.unknownFiles, 0)

  await f.service.markSessionTracked('delegated', f.cwd)
  const delegated = { agent: {}, sessionId: 'delegated' }
  f.service.install(delegated, { sessionId: 'delegated', cwd: f.cwd })
  await delegated.agent.beforeToolCall({
    toolCall: { name: 'call_tool' },
    args: { name: 'read', arguments: { path: 'example.txt' } },
    context: { tools: [] },
  })
  assert.equal((await f.service.summary('delegated', f.cwd)).status, 'partial')
})

test('被拒绝的工具不降级，缺失目标的写工具不误报完整覆盖', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('blocked', f.cwd)
  const blocked = { agent: { beforeToolCall: async () => ({ block: true }) }, sessionId: 'blocked' }
  f.service.install(blocked, { sessionId: 'blocked', cwd: f.cwd })
  await blocked.agent.beforeToolCall({ toolCall: { name: 'bash' }, args: {} })
  assert.equal((await f.service.summary('blocked', f.cwd)).status, 'known')

  await f.service.markSessionTracked('invalid-write', f.cwd)
  const invalid = { agent: {}, sessionId: 'invalid-write' }
  f.service.install(invalid, { sessionId: 'invalid-write', cwd: f.cwd })
  await invalid.agent.beforeToolCall({ toolCall: { name: 'write' }, args: {} })
  assert.equal((await f.service.summary('invalid-write', f.cwd)).status, 'partial')
})

test('索引起始工作区与当前工作区不同时，以及旧索引缺元数据时均保守降级', async (t) => {
  const f = await fixture(t)
  const otherCwd = await mkdtemp(join(f.dataDir, 'moved-workspace-'))
  await f.service.markSessionTracked('workspace-switch', f.cwd)
  assert.equal((await f.service.summary('workspace-switch', f.cwd)).status, 'known')
  assert.equal((await f.service.summary('workspace-switch', otherCwd)).status, 'partial')

  await runWriteTool(f, 'workspace-with-edits', 'same-name.txt', 'first\n')
  await writeFile(join(otherCwd, 'same-name.txt'), 'different\n')
  assert.equal((await f.service.summary('workspace-with-edits', otherCwd)).status, 'partial')

  await f.service.markSessionTracked('legacy', f.cwd)
  await writeFile(f.service.indexPath('legacy'), JSON.stringify({ entries: [] }))
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  assert.equal((await revived.summary('legacy', f.cwd)).status, 'partial')

  await runWriteTool(f, 'persist-cwd', 'persist.txt', 'written\n')
  assert.equal((await revived.summary('persist-cwd', f.cwd)).status, 'known')
})

test('写工具切换工作区后持久降级，返回旧工作区也不误报完整覆盖', async (t) => {
  const f = await fixture(t)
  const otherCwd = await mkdtemp(join(f.dataDir, 'tool-workspace-'))
  await f.service.markSessionTracked('tool-cwd-switch', f.cwd)
  await f.service.run(
    { sessionId: 'tool-cwd-switch', cwd: otherCwd, name: 'write', args: { path: 'same.txt' } },
    () => writeFile(join(otherCwd, 'same.txt'), 'second workspace\n'),
  )

  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  assert.equal((await revived.summary('tool-cwd-switch', f.cwd)).status, 'partial')
  assert.equal((await revived.summary('tool-cwd-switch', otherCwd)).status, 'partial')
})

test('首次快照失败先持久降级，再允许写工具执行', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('capture-failure', f.cwd)
  f.service.captureBefore = async () => {
    throw new Error('snapshot unavailable')
  }
  let executed = false
  await f.service.run(
    { sessionId: 'capture-failure', cwd: f.cwd, name: 'write', args: { path: 'result.txt' } },
    async () => {
      executed = true
      await writeFile(join(f.cwd, 'result.txt'), 'written\n')
    },
  )
  assert.equal(executed, true)
  const revived = new SessionFileChangesService({ dataDir: f.dataDir, warn: () => {} })
  assert.equal((await revived.summary('capture-failure', f.cwd)).status, 'partial')
})

test('共享写工具实例不能把第二会话的调用归给第一会话', async (t) => {
  const f = await fixture(t)
  await f.service.markSessionTracked('first-owner', f.cwd)
  await f.service.markSessionTracked('second-owner', f.cwd)
  const tool = { name: 'write', execute: async () => ({ ok: true }) }
  const prepare = async (sessionId) => {
    const session = { agent: {}, sessionId }
    f.service.install(session, { sessionId, cwd: f.cwd })
    await session.agent.beforeToolCall({
      toolCall: { name: 'write' },
      args: { path: 'shared.txt' },
      context: { tools: [tool] },
    })
  }
  await prepare('first-owner')
  await prepare('second-owner')
  assert.equal((await f.service.summary('first-owner', f.cwd)).status, 'partial')
  assert.equal((await f.service.summary('second-owner', f.cwd)).status, 'partial')
})
