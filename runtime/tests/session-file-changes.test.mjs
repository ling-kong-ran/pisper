// 会话文件变更服务测试：无 VCS 环境下的快照、diff、撤销与审批。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
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
