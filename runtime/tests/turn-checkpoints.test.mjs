// 按轮回退文件：write/edit 执行前记快照，回退到某条消息的时间点。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, stat, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { MAX_CHECKPOINT_FILE_BYTES, TurnCheckpointService } from '../services/turn-checkpoints.mjs'

async function setup(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-turn-checkpoints-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const cwd = join(directory, 'ws')
  await mkdir(cwd)
  let clock = 1000
  const service = new TurnCheckpointService({
    dataDir: join(directory, 'data'),
    now: () => (clock += 1),
  })
  // 模拟一次 write 工具：执行前快照，然后写入。
  const write = async (path, content) => {
    await service.capture('s1', cwd, join(cwd, path))
    await mkdir(join(cwd, path, '..'), { recursive: true })
    await writeFile(join(cwd, path), content)
  }
  const turn = async () => {
    clock += 10
    service.markTurn('s1', clock)
    // markTurn 排在快照队列里；等它生效。
    await service.serialize('s1', async () => {})
    return clock
  }
  const read = (path) => readFile(join(cwd, path), 'utf8')
  return { directory, cwd, service, write, turn, read }
}

test('rewinding restores every file to its content at the chosen turn', async (t) => {
  const { cwd, service, write, turn, read } = await setup(t)
  await writeFile(join(cwd, 'a.txt'), 'a0')
  const turn1 = await turn()
  await write('a.txt', 'a1')
  await write('a.txt', 'a1b')
  await write('new.txt', 'n1')
  const turn2 = await turn()
  await write('a.txt', 'a2')
  await write('new.txt', 'n2')
  await write('dir/deep.txt', 'd2')

  // 回到第二轮之前：a 回到 a1b，new 回到 n1，第二轮新建的 deep 删除。
  assert.deepEqual(await service.preview('s1', turn2), {
    files: [
      { path: 'a.txt', action: 'restore' },
      { path: 'dir/deep.txt', action: 'delete' },
      { path: 'new.txt', action: 'restore' },
    ],
    incomplete: false,
  })
  const second = await service.restore('s1', turn2)
  assert.deepEqual(second.restored, ['a.txt', 'new.txt'])
  assert.deepEqual(second.deleted, ['dir/deep.txt'])
  assert.deepEqual([await read('a.txt'), await read('new.txt')], ['a1b', 'n1'])
  assert.equal(await stat(join(cwd, 'dir/deep.txt')).catch(() => null), null)

  // 再回到第一轮之前：a 回到最初内容，第一轮新建的 new 删除。
  const first = await service.restore('s1', turn1)
  assert.deepEqual([first.restored, first.deleted], [['a.txt'], ['new.txt']])
  assert.equal(await read('a.txt'), 'a0')
  // 已经是目标内容的文件不出现在预览里。
  assert.deepEqual((await service.preview('s1', turn1)).files, [])
})

// 回退不盖掉别人的修改：本会话最后一次写下之后又被改过的文件不回退，单独列出来。
test('rewinding leaves alone files someone else changed after this session last wrote them', async (t) => {
  const { cwd, service, turn, read } = await setup(t)
  await writeFile(join(cwd, 'a.txt'), 'a0')
  const at = await turn()
  // 经钩子写入：执行前记快照，执行后记下写下的内容。
  const session = { agent: {} }
  service.install(session, { sessionId: 's1', cwd })
  const write = async (path, content) => {
    const context = { toolCall: { id: path, name: 'write' }, args: { path, content } }
    await session.agent.beforeToolCall(context)
    await writeFile(join(cwd, path), content)
    await session.agent.afterToolCall({ ...context, result: { content: [] }, isError: false })
  }
  await write('a.txt', 'a1')
  await write('b.txt', 'b1')
  await writeFile(join(cwd, 'a.txt'), 'changed by another session')

  assert.deepEqual(await service.preview('s1', at), {
    files: [
      { path: 'a.txt', action: 'unavailable', reason: 'changed' },
      { path: 'b.txt', action: 'delete' },
    ],
    incomplete: false,
  })
  const result = await service.restore('s1', at)
  assert.deepEqual(result.deleted, ['b.txt'])
  assert.deepEqual(result.unavailable, [{ path: 'a.txt', reason: 'changed' }])
  assert.equal(await read('a.txt'), 'changed by another session')
  // 回退自己写下的内容不算别人的修改：再回退一次没有可做的。
  assert.deepEqual((await service.restore('s1', at)).deleted, [])
})

test('files that cannot be snapshotted are reported instead of silently skipped', async (t) => {
  const { directory, cwd, service, write, turn } = await setup(t)
  await writeFile(join(cwd, 'big.bin'), Buffer.alloc(MAX_CHECKPOINT_FILE_BYTES + 1))
  const at = await turn()
  await write('big.bin', 'small now')
  // 工作区外的路径不记快照，也不会被回退写到外面。
  await service.capture('s1', cwd, join(directory, 'outside.txt'))
  assert.deepEqual(await service.preview('s1', at), {
    files: [{ path: 'big.bin', action: 'unavailable', reason: 'too-large' }],
    incomplete: false,
  })
  const result = await service.restore('s1', at)
  assert.deepEqual(result.unavailable, [{ path: 'big.bin', reason: 'too-large' }])
})

test('a parent directory swapped for a symlink out of the workspace is never written through', async (t) => {
  const { directory, cwd, service, write, turn } = await setup(t)
  await mkdir(join(cwd, 'sub'))
  await writeFile(join(cwd, 'sub/file.txt'), 'before')
  const at = await turn()
  await write('sub/file.txt', 'after')
  const outside = join(directory, 'outside')
  await mkdir(outside)
  await rm(join(cwd, 'sub'), { recursive: true })
  await symlink(outside, join(cwd, 'sub'))
  const result = await service.restore('s1', at)
  assert.deepEqual(result.unavailable, [{ path: 'sub/file.txt', reason: 'path' }])
  assert.equal(await stat(join(outside, 'file.txt')).catch(() => null), null)
})

test('snapshots survive a restart and rewinding twice in a row is stable', async (t) => {
  const { directory, cwd, service, write, turn, read } = await setup(t)
  await writeFile(join(cwd, 'a.txt'), 'a0')
  const at = await turn()
  await write('a.txt', 'a1')
  const restarted = new TurnCheckpointService({ dataDir: join(directory, 'data') })
  await restarted.restore('s1', at)
  assert.equal(await read('a.txt'), 'a0')
  const again = await restarted.restore('s1', at)
  assert.deepEqual([again.restored, again.deleted], [[], []])
  await service.clear('s1')
  assert.deepEqual(
    (await new TurnCheckpointService({ dataDir: join(directory, 'data') }).preview('s1', 0)).files,
    [],
  )
})

// 真实 Runtime：模型两轮各用 write 工具写同一个文件，回退到第一轮结束时与第一轮之前。
test('a real runtime rewinds files written by the agent to a chosen turn', async (t) => {
  const { createServer } = await import('node:http')
  const { AgentRuntimeService } = await import('../runtime/agent-runtime.mjs')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-turn-rewind-'))
  const workspace = join(directory, 'ws')
  await mkdir(workspace)
  const chunk = (delta, finish_reason = null) => ({
    id: 'test',
    object: 'chat.completion.chunk',
    created: 0,
    model: 'fixture',
    choices: [{ index: 0, delta, finish_reason }],
  })
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const parts = []
    for await (const part of request) parts.push(part)
    const body = JSON.parse(Buffer.concat(parts).toString('utf8'))
    const last = body.messages.at(-1)
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = (value) => reply.write(`data: ${JSON.stringify(value)}\n\n`)
    const text = typeof last.content === 'string' ? last.content : JSON.stringify(last.content)
    if (last.role === 'tool') {
      send(chunk({ role: 'assistant', content: 'done' }))
      send(chunk({}, 'stop'))
    } else {
      const version = text.includes('v2') ? 'v2' : 'v1'
      send(
        chunk({
          role: 'assistant',
          tool_calls: [
            {
              index: 0,
              id: `call_${version}`,
              type: 'function',
              function: {
                name: 'write',
                arguments: JSON.stringify({ path: 'notes.txt', content: version }),
              },
            },
          ],
        }),
      )
      send(chunk({}, 'tool_calls'))
    }
    reply.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: {
        'test-rewind': {
          api: 'openai-completions',
          baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
          models: [
            {
              id: 'fixture',
              name: 'Fixture',
              reasoning: false,
              input: ['text'],
              contextWindow: 200000,
              maxTokens: 8192,
            },
          ],
        },
      },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({ 'test-rewind': { type: 'api_key', key: 'test-only-rewind-key' } }),
  )
  runtime = new AgentRuntimeService({ cwd: workspace, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const created = await runtime.createSession('回退', workspace)
  await runtime.setSessionModel(created.id, 'test-rewind', 'fixture')
  await runtime.setSessionExecutionMode(created.id, 'full-access')
  await runtime.streamPrompt({ sessionId: created.id, message: '写入 v1', send: () => {} })
  await runtime.streamPrompt({ sessionId: created.id, message: '改成 v2', send: () => {} })
  assert.equal(await readFile(join(workspace, 'notes.txt'), 'utf8'), 'v2')

  const manager = (await runtime.getExistingSession(created.id)).session.sessionManager
  const messages = manager.getEntries().filter((entry) => entry.type === 'message')
  const firstUser = messages.find((entry) => entry.message.role === 'user')
  const firstBoundary = messages.find(
    (entry) => entry.message.role === 'assistant' && entry.message.stopReason === 'stop',
  )

  // 回到第一轮结束时：文件回到 v1。
  assert.deepEqual(await runtime.previewFileRewind(created.id, firstBoundary.id), {
    files: [{ path: 'notes.txt', action: 'restore' }],
    incomplete: false,
  })
  const toTurn1 = await runtime.rewindFiles(created.id, firstBoundary.id)
  assert.deepEqual(toTurn1.restored, ['notes.txt'])
  assert.equal(await readFile(join(workspace, 'notes.txt'), 'utf8'), 'v1')
  // 回到第一条消息之前：文件当时不存在，删除。
  const toStart = await runtime.rewindFiles(created.id, firstUser.id)
  assert.deepEqual(toStart.deleted, ['notes.txt'])
  assert.equal(await stat(join(workspace, 'notes.txt')).catch(() => null), null)
  await assert.rejects(runtime.rewindFiles(created.id, 'missing'), /不存在/)
})
