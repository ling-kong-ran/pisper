// L0 原文层与批量抽取：模型都是假的，不依赖网络。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { ConversationMemoryCapture } from '../runtime/conversation-memory-capture.mjs'
import { extractBatchMemories } from '../services/memory/conversation-memory.mjs'
import { LocalMemoryRuntime } from '../services/memory/local-memory-runtime.mjs'
import { pruneEpisodes } from '../services/memory/memory-episodes.mjs'

const MINUTE_MS = 60_000

async function withMemory(run) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-episodes-'))
  const cwd = join(directory, 'project')
  await mkdir(cwd)
  const clock = { now: Date.parse('2026-11-05T12:00:00.000Z') }
  const memory = new LocalMemoryRuntime({
    path: join(directory, 'memory.sqlite'),
    cwd,
    now: () => clock.now,
  })
  try {
    await memory.init()
    const spaceId = await memory.ensureWorkspaceSpace(cwd)
    await run({ memory, cwd, spaceId, clock })
  } finally {
    await memory.dispose()
    await rm(directory, { recursive: true, force: true })
  }
}

// 假的模型入口：按顺序取回复，用完了回空数组。
function fakeModel(replies = [], { available = true } = {}) {
  const calls = []
  return {
    calls,
    available: async ({ allowMain = false } = {}) => {
      assert.equal(allowMain, false)
      return available
    },
    complete: async (request) => {
      calls.push(request)
      const reply = replies.length ? replies.shift() : { text: '[]' }
      return typeof reply === 'function' ? reply(request) : reply
    },
  }
}

function captureFor(memory, modelClient, overrides = {}) {
  const diagnostics = []
  const capture = new ConversationMemoryCapture({
    getModelRuntime: () => null,
    waitForInitialization: async () => {},
    memory,
    recordUsage: async () => {},
    modelClient,
    reportFailure: (value) => diagnostics.push(value),
    ...overrides,
  })
  return { capture, diagnostics }
}

const ago = (clock, minutes) => new Date(clock.now - minutes * MINUTE_MS).toISOString()

function addTurns(memory, spaceId, clock, sessionId, count, { minutesAgo = 60, text = '第' } = {}) {
  const ids = []
  for (let index = 0; index < count; index += 1) {
    ids.push(
      memory.recordEpisode({
        sessionId,
        spaceId,
        user: `${text} ${index} 轮的用户消息`,
        assistant: `${text} ${index} 轮的回复`,
        at: ago(clock, minutesAgo - index),
      }).id,
    )
  }
  return ids
}

const episodeCount = (memory, sessionId) =>
  memory
    .requireDb()
    .prepare('SELECT COUNT(*) AS c FROM memory_episodes WHERE session_id = ?')
    .get(sessionId).c

test('a turn is redacted before it is stored, and capped at 8k characters in total', async () => {
  await withMemory(async ({ memory, spaceId }) => {
    const { id } = memory.recordEpisode({
      sessionId: 's1',
      spaceId,
      user: `请用这个配置 apiKey: super-secret-token-value ${'甲'.repeat(9000)}`,
      assistant: `authorization: Bearer assistant-secret-value ${'乙'.repeat(9000)}`,
    })
    const row = memory.requireDb().prepare('SELECT * FROM memory_episodes WHERE id = ?').get(id)
    assert.doesNotMatch(row.user_text, /super-secret-token-value/)
    assert.doesNotMatch(row.assistant_text, /assistant-secret-value/)
    assert.match(row.user_text, /REDACTED SECRET/)
    assert.ok(row.user_text.length + row.assistant_text.length <= 8000)
    assert.ok(row.bytes >= row.user_text.length)
  })
})

test('truncation never leaves half of a surrogate pair behind', async () => {
  await withMemory(async ({ memory, spaceId }) => {
    const { id } = memory.recordEpisode({
      sessionId: 's1',
      spaceId,
      user: `a${'😀'.repeat(5000)}`,
      assistant: 'ok',
    })
    const row = memory.requireDb().prepare('SELECT * FROM memory_episodes WHERE id = ?').get(id)
    assert.equal(row.user_text.isWellFormed(), true)
  })
})

test('stored conversations can be found by part of a Chinese word, separately from facts', async () => {
  await withMemory(async ({ memory, spaceId }) => {
    memory.recordEpisode({
      sessionId: 's1',
      spaceId,
      user: '上次那个网关超时的报错怎么解决的',
      assistant: '把请求改成流式就好了',
    })
    const found = memory.search('网关超时', {
      spaceIds: [spaceId],
      kinds: ['episode'],
      trackAccess: false,
    })
    assert.equal(found.length, 1)
    assert.equal(found[0].kind, 'episode')
    assert.match(found[0].content, /流式/)
    assert.equal(memory.search('网关超时', { spaceIds: [spaceId], trackAccess: false }).length, 0)
  })
})

test('without a spaceId a turn goes to the project of its cwd; an unknown project is skipped', async () => {
  await withMemory(async ({ memory, cwd, spaceId }) => {
    const { id } = memory.recordEpisode({ sessionId: 's1', cwd, user: '项目里的一轮对话' })
    assert.equal(
      memory.requireDb().prepare('SELECT space_id FROM memory_episodes WHERE id = ?').get(id)
        .space_id,
      spaceId,
    )
    assert.equal(
      memory.recordEpisode({ sessionId: 's1', cwd: join(cwd, 'other'), user: 'x' }),
      null,
    )
    assert.equal(memory.recordEpisode({ sessionId: 's1', cwd, user: '', assistant: '' }), null)
  })
})

test('a session is due once it has been idle long enough or has enough turns', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 'recent', 3, { minutesAgo: 3 })
    addTurns(memory, spaceId, clock, 'idle', 2, { minutesAgo: 40 })
    addTurns(memory, spaceId, clock, 'busy', 10, { minutesAgo: 5 })
    const due = memory.pendingEpisodeBatches({ idleMs: 10 * MINUTE_MS, minTurns: 10 })
    assert.deepEqual(due.map((batch) => batch.sessionId).sort(), ['busy', 'idle'])
    const idle = due.find((batch) => batch.sessionId === 'idle')
    assert.equal(idle.spaceId, spaceId)
    assert.deepEqual(
      idle.episodes.map((episode) => episode.user),
      ['第 0 轮的用户消息', '第 1 轮的用户消息'],
    )
    assert.equal(memory.pendingEpisodeBatches({ limit: 1 }).length, 1)
    assert.equal(
      memory.pendingEpisodeBatches({ maxTurns: 4 }).find((b) => b.sessionId === 'busy').episodes
        .length,
      4,
    )
    // 压缩前不看空闲和累计，只看这一个会话。
    const forced = memory.pendingEpisodeBatches({ sessionId: 'recent', force: true })
    assert.deepEqual(
      forced.map((batch) => batch.sessionId),
      ['recent'],
    )
  })
})

test('the batch prompt asks for no review and goes through the auxiliary-only path', async () => {
  const model = fakeModel()
  const result = await extractBatchMemories({
    modelClient: model,
    turns: [{ id: 'e1', user: '我们用 pnpm 吧', assistant: '好', at: '2026-11-01T00:00:00.000Z' }],
  })
  assert.deepEqual(result, { memories: [] })
  const request = model.calls[0]
  assert.equal(request.purpose, 'extract')
  assert.equal(request.allowMain, false)
  assert.match(request.system, /Nobody reviews/)
  assert.match(request.user, /2026-11-01/)
  assert.match(request.user, /我们用 pnpm 吧/)
})

test('long batches are shared out across turns so the prompt stays bounded', async () => {
  const model = fakeModel()
  await extractBatchMemories({
    modelClient: model,
    turns: Array.from({ length: 30 }, (_, index) => ({
      id: `e${index}`,
      user: '用'.repeat(8000),
      assistant: '答'.repeat(8000),
      at: '2026-11-01T00:00:00.000Z',
    })),
  })
  assert.ok(model.calls[0].user.length < 30_000)
})

test('evidence must be a verbatim quote of some turn; whose words it is comes from the text', async () => {
  const items = [
    { title: '用户说的', content: '项目用 pnpm', topic: 'p.pm', evidence: '我们改用 pnpm 吧' },
    {
      title: '助手说的',
      content: '已经部署到生产',
      topic: 'p.dep',
      evidence: '已经部署到生产环境',
    },
    { title: '编造的', content: '用户喜欢 Rust', topic: 'p.rust', evidence: '用户明确说喜欢 Rust' },
    { title: '太短的', content: '不够格', topic: 'p.short', evidence: 'pn' },
    { title: '脱敏的', content: '不要', topic: 'p.sec', evidence: 'key: [REDACTED SECRET]' },
    { title: '', content: '没标题', topic: 'p.none', evidence: '我们改用 pnpm 吧' },
  ]
  const result = await extractBatchMemories({
    modelClient: fakeModel([{ text: `说明：\n${JSON.stringify(items)}` }]),
    turns: [
      { id: 'e1', user: '先看看', assistant: '已经部署到生产环境', at: '2026-11-01T00:00:00.000Z' },
      {
        id: 'e2',
        user: '我们改用 pnpm 吧，key: [REDACTED SECRET]',
        assistant: '好',
        at: '2026-11-02T00:00:00.000Z',
        spaceId: 'project-x',
        cwd: '/w',
      },
    ],
  })
  assert.deepEqual(
    result.memories.map((item) => [item.title, item.evidenceRole, item.episodeId]),
    [
      ['用户说的', 'user', 'e2'],
      ['助手说的', 'assistant', 'e1'],
    ],
  )
  assert.equal(result.memories[0].sourceTimestamp, '2026-11-02T00:00:00.000Z')
  assert.equal(result.memories[0].projectSpaceId, 'project-x')
})

test('valid_from and expires_at are kept only when they parse and make sense', async () => {
  const now = Date.parse('2026-11-05T00:00:00.000Z')
  const turn = {
    id: 'e1',
    user: '从九月起改用 pnpm，这周先关掉缓存',
    assistant: '',
    at: '2026-11-04T00:00:00.000Z',
  }
  const item = (extra) => ({ title: 't', content: 'c', evidence: '从九月起改用 pnpm', ...extra })
  const { memories } = await extractBatchMemories({
    now,
    modelClient: fakeModel([
      {
        text: JSON.stringify([
          item({ topic: 'a.a', valid_from: '2026-09-01', expires_at: '2026-11-09' }),
          item({ topic: 'a.b', valid_from: '上个月', expires_at: 'soon' }),
          item({ topic: 'a.c', valid_from: '2027-01-01', expires_at: '2026-01-01' }),
        ]),
      },
    ]),
    turns: [turn],
  })
  assert.equal(memories[0].validFrom, '2026-09-01T00:00:00.000Z')
  assert.equal(memories[0].expiresAt, '2026-11-09T00:00:00.000Z')
  assert.deepEqual([memories[1].validFrom, memories[1].expiresAt], ['', ''])
  assert.deepEqual([memories[2].validFrom, memories[2].expiresAt], ['', ''])
})

test('the number of items per batch grows with the turns but has a ceiling', async () => {
  const quote = '这是一句可以当证据的话'
  const items = Array.from({ length: 40 }, (_, index) => ({
    title: `条目 ${index}`,
    content: `内容 ${index}`,
    topic: `t.item.${index}`,
    evidence: quote,
  }))
  const count = async (turnCount) =>
    (
      await extractBatchMemories({
        modelClient: fakeModel([{ text: JSON.stringify(items) }]),
        turns: Array.from({ length: turnCount }, (_, index) => ({
          id: `e${index}`,
          user: quote,
          assistant: '',
          at: '2026-11-01T00:00:00.000Z',
        })),
      })
    ).memories.length
  assert.equal(await count(1), 3)
  assert.equal(await count(9), 6)
  assert.equal(await count(30), 12)
})

test('idle sessions are extracted into live memories and the cursor stops repeats', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    const [first] = addTurns(memory, spaceId, clock, 's1', 2, { text: '我们决定用 Vite' })
    const model = fakeModel([
      {
        text: JSON.stringify([
          {
            title: '构建工具',
            content: '项目构建工具用 Vite',
            topic: 'project.build.tool',
            type: 'decision',
            scope: 'project',
            evidence: '我们决定用 Vite 0 轮',
            valid_from: '2026-09-01',
          },
          {
            title: '回复语言',
            content: '默认用中文回复',
            topic: 'user.language',
            type: 'preference',
            scope: 'global',
            evidence: '我们决定用 Vite 1 轮',
          },
        ]),
      },
    ])
    const { capture, diagnostics } = captureFor(memory, model)
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 2 })
    const [fact] = memory.listMemories({ spaceId })
    assert.equal(fact.status, 'active')
    assert.equal(fact.sourceType, 'conversation')
    assert.equal(fact.evidenceRole, 'user')
    assert.equal(fact.authority, 90)
    assert.equal(fact.episodeId, first)
    assert.equal(fact.sessionId, 's1')
    assert.equal(fact.validFrom, '2026-09-01T00:00:00.000Z')
    assert.equal(memory.listMemories({ spaceId: 'global' })[0].title, '回复语言')
    // 同一段对话不再抽第二次。
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.equal(model.calls.length, 1)
    assert.deepEqual(diagnostics, [])
    // 之后的新轮次才会再进批量。
    addTurns(memory, spaceId, clock, 's1', 1, { minutesAgo: 30, text: '后来' })
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 1 })
    assert.equal(model.calls.length, 2)
    await capture.dispose()
  })
})

test('a batch with nothing worth keeping still moves the cursor', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 2)
    const model = fakeModel([{ text: '[]' }])
    const { capture } = captureFor(memory, model)
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 2 })
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.equal(model.calls.length, 1)
    await capture.dispose()
  })
})

test('without an auxiliary model nothing runs and the turns wait', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 2)
    const model = fakeModel([], { available: false })
    const { capture } = captureFor(memory, model)
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.deepEqual(await capture.flushSessionBeforeCompaction('s1'), { processed: 0 })
    assert.equal(model.calls.length, 0)
    assert.equal(memory.pendingEpisodeBatches().length, 1)
    // 没传模型入口也一样。
    const bare = captureFor(memory, null)
    assert.deepEqual(await bare.capture.flushIdleSessions(), { processed: 0 })
    await capture.dispose()
    await bare.capture.dispose()
  })
})

test('a model failure keeps the cursor for a retry and stops this round', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 'a', 1)
    addTurns(memory, spaceId, clock, 'b', 1)
    const model = fakeModel([{ errorCode: 'model_error' }, { text: '[]' }, { text: '[]' }])
    const { capture, diagnostics } = captureFor(memory, model)
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.equal(model.calls.length, 1)
    assert.deepEqual(
      diagnostics.map((item) => item.code),
      ['model_error'],
    )
    assert.match(diagnostics[0].session, /^[a-f0-9]{12}$/)
    assert.equal(memory.pendingEpisodeBatches().length, 2)
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 2 })
    assert.equal(memory.pendingEpisodeBatches().length, 0)
    await capture.dispose()
  })
})

for (const reply of [
  { text: 'not json at all' },
  { errorCode: 'empty_response' },
  { errorCode: 'token_limit' },
]) {
  test(`an unusable reply (${reply.errorCode || 'invalid_response'}) moves the cursor, it would only cost money again`, async () => {
    await withMemory(async ({ memory, spaceId, clock }) => {
      addTurns(memory, spaceId, clock, 's1', 1)
      const model = fakeModel([reply])
      const { capture, diagnostics } = captureFor(memory, model)
      assert.deepEqual(await capture.flushIdleSessions(), { processed: 1 })
      assert.deepEqual(
        diagnostics.map((item) => item.code),
        [reply.errorCode || 'invalid_response'],
      )
      assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
      assert.equal(model.calls.length, 1)
      await capture.dispose()
    })
  })
}

test('diagnostics never carry the conversation', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 'private/session', 1, { text: '私密内容' })
    const model = fakeModel([
      async () => {
        throw new Error('私密内容 from SDK')
      },
    ])
    const { capture, diagnostics } = captureFor(memory, model)
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.deepEqual(
      diagnostics.map((item) => item.code),
      ['model_error'],
    )
    assert.doesNotMatch(JSON.stringify(diagnostics), /私密内容|private/)
    await capture.dispose()
  })
})

test('a turn handled by the immediate path is not extracted again by the batch', async () => {
  await withMemory(async ({ memory, cwd, clock }) => {
    const reply = JSON.stringify([
      {
        title: '回复语言',
        content: '默认用中文回复',
        topic: 'user.language',
        type: 'preference',
        scope: 'global',
        evidence: '记住以后默认用中文回复',
        expires_at: '2099-01-01',
      },
    ])
    const model = fakeModel()
    const { capture, diagnostics } = captureFor(memory, model, {
      getModelRuntime: () => ({
        completeSimple: async () => ({
          content: [{ type: 'text', text: reply }],
          usage: null,
          timestamp: clock.now,
          stopReason: 'stop',
        }),
      }),
    })
    const stored = await capture.capture({
      sessionId: 's1',
      cwd,
      model: {},
      user: '记住以后默认用中文回复',
      assistant: '好的',
      sourceTimestamp: ago(clock, 60),
      recordTurn: true,
    })
    assert.equal(stored.length, 1)
    assert.equal(stored[0].expiresAt, '2099-01-01T00:00:00.000Z')
    assert.equal(stored[0].evidenceRole, 'user')
    assert.ok(stored[0].episodeId)
    assert.equal(episodeCount(memory, 's1'), 1)
    assert.deepEqual(diagnostics, [])
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.equal(model.calls.length, 0)
    await capture.dispose()
  })
})

test('when the immediate path fails the batch gets another try at that turn', async () => {
  await withMemory(async ({ memory, cwd, clock }) => {
    const model = fakeModel()
    const { capture, diagnostics } = captureFor(memory, model, {
      getModelRuntime: () => ({
        completeSimple: async () => ({ content: [], stopReason: 'error', errorMessage: 'x' }),
      }),
    })
    await capture.capture({
      sessionId: 's1',
      cwd,
      model: {},
      user: '记住以后默认用中文回复',
      assistant: '好的',
      sourceTimestamp: ago(clock, 60),
      recordTurn: true,
    })
    assert.deepEqual(
      diagnostics.map((item) => item.code),
      ['model_error'],
    )
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 1 })
    assert.equal(model.calls.length, 1)
    await capture.dispose()
  })
})

test('an ordinary turn is stored without any model call', async () => {
  await withMemory(async ({ memory, cwd }) => {
    let calls = 0
    const { capture } = captureFor(memory, fakeModel(), {
      getModelRuntime: () => ({
        completeSimple: async () => {
          calls += 1
          return {}
        },
      }),
    })
    const result = await capture.capture({
      sessionId: 's1',
      cwd,
      model: {},
      user: '帮我解释这个函数',
      assistant: '它做了这些事',
      recordTurn: true,
    })
    assert.deepEqual(result, [])
    assert.equal(calls, 0)
    assert.equal(episodeCount(memory, 's1'), 1)
    await capture.dispose()
  })
})

test('a failed turn write does not stop what the user asked to remember', async () => {
  const stored = []
  const { capture, diagnostics } = captureFor(
    {
      ensureWorkspaceSpace: async () => 'project',
      recordEpisode: () => {
        throw new Error('private path')
      },
      remember: (value) => {
        stored.push(value)
        return value
      },
    },
    null,
    {
      getModelRuntime: () => ({
        completeSimple: async () => ({
          content: [
            {
              type: 'text',
              text: JSON.stringify([
                {
                  title: '语言',
                  content: '用中文',
                  topic: 'u.lang',
                  evidence: '记住默认用中文回复',
                },
              ]),
            },
          ],
          stopReason: 'stop',
        }),
      }),
    },
  )
  const result = await capture.capture({
    sessionId: 's1',
    cwd: '.',
    model: {},
    user: '记住默认用中文回复',
    assistant: '好',
    recordTurn: true,
  })
  assert.equal(result.length, 1)
  assert.equal(stored[0].episodeId, '')
  assert.deepEqual(
    diagnostics.map((item) => item.code),
    ['episode_write_failed'],
  )
  await capture.dispose()
})

test('before compaction a session is flushed even if it is not idle; failures stay quiet', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 2, { minutesAgo: 2 })
    const model = fakeModel([{ errorCode: 'model_error' }, { text: '[]' }])
    const { capture, diagnostics } = captureFor(memory, model)
    assert.equal(memory.pendingEpisodeBatches().length, 0)
    assert.deepEqual(await capture.flushSessionBeforeCompaction('s1'), { processed: 0 })
    assert.deepEqual(
      diagnostics.map((item) => item.code),
      ['model_error'],
    )
    assert.deepEqual(await capture.flushSessionBeforeCompaction('s1'), { processed: 2 })
    assert.deepEqual(await capture.flushSessionBeforeCompaction('no-such-session'), {
      processed: 0,
    })
    await capture.dispose()
  })
})

test('compaction flush swallows a broken memory store', async () => {
  const { capture, diagnostics } = captureFor(
    {
      pendingEpisodeBatches: () => {
        throw new Error('private database path')
      },
    },
    fakeModel(),
  )
  assert.deepEqual(await capture.flushSessionBeforeCompaction('s1'), { processed: 0 })
  assert.deepEqual(
    diagnostics.map((item) => item.code),
    ['flush_failed'],
  )
  await capture.dispose()
})

test('the timer and the compaction hook never spend twice on the same session', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 2)
    const release = Promise.withResolvers()
    const model = fakeModel([() => release.promise])
    const { capture } = captureFor(memory, model)
    const running = capture.flushIdleSessions()
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(model.calls.length, 1)
    assert.deepEqual(await capture.flushSessionBeforeCompaction('s1'), { processed: 0 })
    assert.equal(model.calls.length, 1)
    release.resolve({ text: '[]' })
    assert.deepEqual(await running, { processed: 2 })
    await capture.dispose()
  })
})

test('shutdown cancels a batch in flight without writing anything late', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 1, { text: '我们决定用 Vite' })
    const late = Promise.withResolvers()
    let seenSignal
    const model = fakeModel([
      ({ signal }) => {
        seenSignal = signal
        return late.promise
      },
    ])
    const { capture, diagnostics } = captureFor(memory, model)
    const running = capture.flushIdleSessions()
    await new Promise((resolve) => setImmediate(resolve))
    await capture.dispose()
    assert.equal(seenSignal.aborted, true)
    assert.deepEqual(await running, { processed: 0 })
    late.resolve({
      text: JSON.stringify([{ title: 't', content: 'c', evidence: '我们决定用 Vite 0 轮' }]),
    })
    await new Promise((resolve) => setImmediate(resolve))
    assert.equal(memory.listMemories({ spaceId }).length, 0)
    assert.equal(capture.tasks.size, 0)
    assert.deepEqual(diagnostics, [])
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
  })
})

test('an external abort signal stops the round and keeps the cursor', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 1)
    const controller = new AbortController()
    const model = fakeModel([
      ({ signal }) =>
        new Promise((resolve) => {
          signal.addEventListener('abort', () => resolve({ errorCode: 'aborted' }), { once: true })
          controller.abort()
        }),
    ])
    const { capture, diagnostics } = captureFor(memory, model)
    assert.deepEqual(await capture.flushIdleSessions({ signal: controller.signal }), {
      processed: 0,
    })
    assert.deepEqual(diagnostics, [])
    assert.equal(memory.pendingEpisodeBatches().length, 1)
    await capture.dispose()
  })
})

test('a stuck model is abandoned at the deadline with only a timeout diagnostic', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 1)
    const model = fakeModel([() => new Promise(() => {})])
    const { capture, diagnostics } = captureFor(memory, model, { timeoutMs: 5 })
    assert.deepEqual(await capture.flushIdleSessions(), { processed: 0 })
    assert.deepEqual(
      diagnostics.map((item) => item.code),
      ['timeout'],
    )
    assert.equal(memory.pendingEpisodeBatches().length, 1)
    await capture.dispose()
  })
})

test('deleting a session removes its turns and index but keeps the facts it produced', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    const [first] = addTurns(memory, spaceId, clock, 'gone', 2, { text: '网关超时' })
    addTurns(memory, spaceId, clock, 'kept', 1, { text: '网关超时' })
    memory.saveSessionCursor('gone', ago(clock, 60))
    const fact = memory.remember({
      spaceId,
      title: '网关',
      content: '请求超过 120 秒会被断开',
      episodeId: first,
      sessionId: 'gone',
    })
    assert.equal(memory.forgetSession('gone'), 2)
    const db = memory.requireDb()
    assert.equal(episodeCount(memory, 'gone'), 0)
    assert.equal(episodeCount(memory, 'kept'), 1)
    assert.equal(
      db.prepare('SELECT 1 FROM memory_session_cursors WHERE session_id = ?').get('gone'),
      undefined,
    )
    assert.equal(db.prepare('SELECT 1 FROM memory_docs WHERE doc_id = ?').get(first), undefined)
    const found = memory.search('网关超时', {
      spaceIds: [spaceId],
      kinds: ['episode'],
      trackAccess: false,
    })
    assert.equal(found.length, 1)
    assert.equal(found[0].sessionId, 'kept')
    assert.equal(memory.getMemory(fact.id).status, 'active')
    assert.equal(memory.getMemory(fact.id).episodeId, '')
    assert.equal(memory.forgetSession('gone'), 0)
  })
})

test('over the size limit the oldest turns go first, along with their index', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    const ids = addTurns(memory, spaceId, clock, 's1', 6, { text: '网关超时' })
    const fact = memory.remember({
      spaceId,
      title: '事实',
      content: '留下来的事实',
      episodeId: ids[0],
    })
    const db = memory.requireDb()
    const total = db.prepare('SELECT SUM(bytes) AS total FROM memory_episodes').get().total
    assert.equal(pruneEpisodes(memory.store, { maxBytes: total }), 0)
    const removed = pruneEpisodes(memory.store, { maxBytes: Math.floor(total / 2) })
    assert.ok(removed >= 3 && removed < 6, `removed ${removed}`)
    const left = db
      .prepare('SELECT id FROM memory_episodes ORDER BY at')
      .all()
      .map((row) => row.id)
    assert.deepEqual(left, ids.slice(removed))
    assert.ok(
      db.prepare('SELECT SUM(bytes) AS total FROM memory_episodes').get().total <= total / 2,
    )
    assert.equal(db.prepare('SELECT 1 FROM memory_docs WHERE doc_id = ?').get(ids[0]), undefined)
    assert.equal(memory.getMemory(fact.id).episodeId, '')
  })
})

test('the cursor only moves forward', async () => {
  await withMemory(async ({ memory, spaceId, clock }) => {
    addTurns(memory, spaceId, clock, 's1', 3)
    memory.saveSessionCursor('s1', ago(clock, 58))
    memory.saveSessionCursor('s1', ago(clock, 60))
    const db = memory.requireDb()
    assert.equal(
      db.prepare('SELECT episode_at FROM memory_session_cursors WHERE session_id = ?').get('s1')
        .episode_at,
      ago(clock, 58),
    )
    assert.equal(memory.pendingEpisodeBatches().length, 0)
  })
})

async function withRuntime(t, run) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-episodes-runtime-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  await run(runtime, directory)
}

const turnsOf = (runtime, sessionId) =>
  runtime.memory
    .requireDb()
    .prepare('SELECT user_text FROM memory_episodes WHERE session_id = ?')
    .all(sessionId)
    .map((row) => row.user_text)

test('the runtime stores what the user said, without injected context or internal messages', async (t) => {
  await withRuntime(t, async (runtime, directory) => {
    const base = { sessionId: 's1', cwd: directory, model: null, assistant: '好的' }
    await runtime.captureConversationMemory({
      ...base,
      user: '解释这个函数\n\n---\nAttachment context (injected by Pisper):\n<pisper_memory_context>旧记忆</pisper_memory_context>',
    })
    await runtime.captureConversationMemory({
      ...base,
      user: '[Pisper internal agent completion]\n子 agent 做完了',
    })
    assert.deepEqual(turnsOf(runtime, 's1'), ['解释这个函数'])
  })
})

test('deleting a session clears its turns; a broken memory store cannot fail the delete', async (t) => {
  await withRuntime(t, async (runtime, directory) => {
    await runtime.captureConversationMemory({
      sessionId: 's1',
      cwd: directory,
      model: null,
      user: '解释这个函数',
      assistant: '好的',
    })
    assert.equal(turnsOf(runtime, 's1').length, 1)
    runtime.sessionLifecycle.deleteSession = async () => true
    assert.equal(await runtime.deleteSession('s1'), true)
    assert.equal(turnsOf(runtime, 's1').length, 0)

    const forget = runtime.memory.forgetSession
    runtime.memory.forgetSession = () => {
      throw new Error('private database path')
    }
    const warnings = t.mock.method(console, 'warn', () => {})
    assert.equal(await runtime.deleteSession('s2'), true)
    assert.equal(warnings.mock.callCount(), 1)
    assert.doesNotMatch(String(warnings.mock.calls[0].arguments), /private/)
    runtime.memory.forgetSession = forget
  })
})

test('a failed session delete leaves the turns alone', async (t) => {
  await withRuntime(t, async (runtime, directory) => {
    await runtime.captureConversationMemory({
      sessionId: 's1',
      cwd: directory,
      model: null,
      user: '解释这个函数',
      assistant: '好的',
    })
    runtime.sessionLifecycle.deleteSession = async () => false
    assert.equal(await runtime.deleteSession('s1'), false)
    assert.equal(turnsOf(runtime, 's1').length, 1)
  })
})

async function runCompaction(t, { enabledTools, initialization, isolatedContext = false, flush }) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-episodes-compaction-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  runtime.initialization = initialization
  const flushed = []
  runtime.memoryCapture = {
    diagnose: (code) => flushed.push(`diagnose:${code}`),
    flushSessionBeforeCompaction: (id) => {
      flushed.push(id)
      return flush()
    },
  }
  const listeners = new Set()
  const session = {
    sessionId: 'session-compaction',
    isStreaming: false,
    model: { provider: 'openai', id: 'gpt-5.4' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier turn', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    async prompt() {
      session.isStreaming = true
      for (const listener of listeners) listener({ type: 'agent_start' })
      for (const listener of listeners) listener({ type: 'compaction_start', reason: 'threshold' })
      session.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Done.' }],
        timestamp: 2,
      })
      session.isStreaming = false
    },
  }
  const value = {
    session,
    cwd: directory,
    name: 'Compaction',
    baseToolNames: [],
    enabledTools,
    isolatedContext,
  }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value
  const events = []
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: '继续',
    send: (event) => events.push(event),
    ...(isolatedContext ? { isolatedContext: true } : {}),
  })
  await new Promise((resolve) => setImmediate(resolve))
  return { flushed, events }
}

test('compaction starts a flush without waiting for it, and a rejection stays contained', async (t) => {
  const never = await runCompaction(t, {
    enabledTools: ['memory_remember'],
    initialization: { memory: 'ready' },
    flush: () => new Promise(() => {}),
  })
  assert.deepEqual(never.flushed, ['session-compaction'])
  assert.ok(never.events.includes('done'))

  const rejected = await runCompaction(t, {
    enabledTools: ['memory_remember'],
    initialization: { memory: 'ready' },
    flush: async () => {
      throw new Error('private')
    },
  })
  assert.deepEqual(rejected.flushed, ['session-compaction', 'diagnose:flush_failed'])
  assert.ok(rejected.events.includes('done'))
})

test('compaction skips the flush when memory is off, not ready or the context is isolated', async (t) => {
  const flush = async () => ({ processed: 0 })
  const disabled = await runCompaction(t, {
    enabledTools: [],
    initialization: { memory: 'ready' },
    flush,
  })
  assert.deepEqual(disabled.flushed, [])
  const notReady = await runCompaction(t, {
    enabledTools: ['memory_remember'],
    initialization: { memory: 'initializing' },
    flush,
  })
  assert.deepEqual(notReady.flushed, [])
  const isolated = await runCompaction(t, {
    enabledTools: ['memory_remember'],
    initialization: { memory: 'ready' },
    isolatedContext: true,
    flush,
  })
  assert.deepEqual(isolated.flushed, [])
})
