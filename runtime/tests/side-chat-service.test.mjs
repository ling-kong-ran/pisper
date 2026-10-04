import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import { SIDE_CHAT_TTL_MS, SideChatService } from '../services/side-chat-service.mjs'

function deferred() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

function fixture() {
  let now = Date.parse('2026-09-27T00:00:00.000Z')
  let writes = 0
  const metadata = {}
  const summaries = new Map([
    [
      'parent',
      {
        id: 'parent',
        cwd: '/fixture',
        model: 'provider/nested/model',
        thinkingLevel: 'high',
        executionMode: 'workspace-write',
        permissionMode: 'ask',
        runMode: 'team',
      },
    ],
  ])
  const protectedIds = new Set()
  const deleted = []
  let beforeDelete = async () => {}
  const service = new SideChatService({
    getMetadata: () => metadata,
    saveMetadata: async () => {
      writes += 1
    },
    getSummary: async (id) => summaries.get(id) || null,
    createSession: async ({ id, cwd, metadata: entry }) => {
      metadata[id] = entry
      const summary = { id, cwd, ...entry }
      summaries.set(id, summary)
      return summary
    },
    deleteSession: (id) =>
      service.deleteWithChildren(id, async () => {
        await beforeDelete(id)
        deleted.push(id)
        delete metadata[id]
        summaries.delete(id)
      }),
    isProtected: (id) => protectedIds.has(id),
    now: () => now,
  })
  return {
    service,
    metadata,
    summaries,
    protectedIds,
    deleted,
    advance: (amount) => {
      now += amount
    },
    setBeforeDelete: (callback) => {
      beforeDelete = callback
    },
    get now() {
      return now
    },
    get writes() {
      return writes
    },
  }
}

test('concurrent side-chat creation is idempotent and only inherits parent configuration', async () => {
  const f = fixture()
  assert.deepEqual(await f.service.get('parent'), {
    session: null,
    expiresAt: null,
    created: false,
  })
  const results = await Promise.all(
    Array.from({ length: 6 }, () => f.service.get('parent', { create: true })),
  )
  assert.equal(new Set(results.map((result) => result.session.id)).size, 1)
  assert.equal(results.filter((result) => result.created).length, 1)
  const result = results[0]
  assert.equal(Date.parse(result.expiresAt), f.now + SIDE_CHAT_TTL_MS)
  for (const key of [
    'cwd',
    'model',
    'thinkingLevel',
    'executionMode',
    'permissionMode',
    'runMode',
  ]) {
    assert.equal(result.session[key], f.summaries.get('parent')[key])
  }
  assert.equal(result.session.parentSessionId, undefined)
  assert.equal(result.session.goal, undefined)
  assert.equal(result.session.team, undefined)
  f.summaries.get('parent').model = 'different/model'
  assert.equal(
    (await f.service.get('parent', { create: true })).session.model,
    'provider/nested/model',
  )
  await assert.rejects(f.service.get(result.session.id, { create: true }), {
    code: 'invalid_side_chat_parent',
  })
  await assert.rejects(f.service.get('missing', { create: true }), { code: 'session_not_found' })
})

test('reading or reopening does not renew idle expiration and expired IDs stay rejected', async () => {
  const f = fixture()
  const initial = await f.service.get('parent', { create: true })
  f.advance(SIDE_CHAT_TTL_MS - 1)
  assert.equal((await f.service.get('parent')).expiresAt, initial.expiresAt)
  assert.equal((await f.service.get('parent', { create: true })).expiresAt, initial.expiresAt)
  assert.equal(f.writes, 0)
  f.advance(1)
  assert.throws(() => f.service.assertAvailable(initial.session.id), {
    code: 'side_chat_expired',
    statusCode: 410,
  })
  assert.deepEqual(await f.service.get('parent'), {
    session: null,
    expiresAt: null,
    created: false,
  })
  assert.throws(() => f.service.assertAvailable(initial.session.id), {
    code: 'side_chat_not_found',
    statusCode: 404,
  })
  const fresh = await f.service.get('parent', { create: true })
  assert.notEqual(fresh.session.id, initial.session.id)
})

test('a running side chat survives expiration and gets a new idle period when the run settles', async () => {
  const f = fixture()
  const { session } = await f.service.get('parent', { create: true })
  f.advance(5_000)
  const finish = await f.service.beginRun(session.id)
  assert.equal(Date.parse(f.metadata[session.id].sideChat.expiresAt), f.now + SIDE_CHAT_TTL_MS)
  f.advance(SIDE_CHAT_TTL_MS * 2)
  await f.service.sweep()
  assert.doesNotThrow(() => f.service.assertAvailable(session.id))
  assert.equal(f.deleted.length, 0)
  await finish()
  assert.equal(Date.parse(f.metadata[session.id].sideChat.expiresAt), f.now + SIDE_CHAT_TTL_MS)
  assert.equal(f.service.reservations.size, 0)
  f.advance(SIDE_CHAT_TTL_MS)
  await f.service.sweep()
  assert.deepEqual(f.deleted, [session.id])
})

test('active child agents protect expired side chats until they settle', async () => {
  const f = fixture()
  const { session } = await f.service.get('parent', { create: true })
  f.protectedIds.add(session.id)
  f.advance(SIDE_CHAT_TTL_MS)
  await f.service.sweep()
  assert.equal(f.deleted.length, 0)
  f.protectedIds.delete(session.id)
  await f.service.touch(session.id)
  await f.service.sweep()
  assert.equal(f.deleted.length, 0)
})

test('expiration reserves deletion before yielding and runtime shutdown drains the sweep', async () => {
  const f = fixture()
  const { session } = await f.service.get('parent', { create: true })
  const deleting = deferred()
  const proceed = deferred()
  f.setBeforeDelete(async () => {
    deleting.resolve()
    await proceed.promise
  })
  f.advance(SIDE_CHAT_TTL_MS)
  const sweeping = f.service.sweep()
  await deleting.promise
  await assert.rejects(f.service.beginRun(session.id), { code: 'side_chat_not_found' })
  let disposed = false
  const disposing = f.service.dispose().then(() => {
    disposed = true
  })
  await Promise.resolve()
  assert.equal(disposed, false)
  proceed.resolve()
  await Promise.all([sweeping, disposing])
  assert.deepEqual(f.deleted, [session.id])
  await f.service.sweep()
  assert.equal(f.deleted.length, 1)
})

async function runtimeFixture(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-side-chat-'))
  const runtimes = []
  const create = () => {
    const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
    runtime.settingsManager = {
      getGlobalSettings: () => ({
        defaultProvider: 'fallback',
        defaultModel: 'default',
        defaultThinkingLevel: 'low',
      }),
    }
    runtimes.push(runtime)
    return runtime
  }
  t.after(async () => {
    for (const runtime of runtimes) await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  return { runtime: create(), create, directory }
}

test('persisted side chats remain lightweight, inherit model changes and stay outside ordinary catalogs', async (t) => {
  const { runtime, directory, create } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  Object.assign(runtime.sessionMeta[parent.id], {
    model: 'provider/nested/model',
    thinkingLevel: 'high',
    executionMode: 'workspace-write',
    permissionMode: 'ask',
    runMode: 'team',
  })
  await runtime.saveSessionMeta()
  const created = await runtime.getSideChat(parent.id, { create: true })
  assert.equal(runtime.sessions.size, 0)
  assert.equal(created.session.model, 'provider/nested/model')
  assert.equal(created.session.thinkingLevel, 'high')
  assert.equal(created.session.runMode, 'team')
  assert.equal(created.session.executionMode, 'workspace-write')
  assert.equal(created.session.goal, null)
  const branch = runtime.pendingSessions.get(created.session.id).manager.getBranch()
  assert.ok(
    branch.some(
      (entry) =>
        entry.type === 'model_change' &&
        entry.provider === 'provider' &&
        entry.modelId === 'nested/model',
    ),
  )
  assert.ok(
    branch.some(
      (entry) => entry.type === 'thinking_level_change' && entry.thinkingLevel === 'high',
    ),
  )
  assert.deepEqual(
    (await runtime.listSessions()).map((item) => item.id),
    [parent.id],
  )
  assert.equal(runtime.sessionLifecycle.getSessionLineage(parent.id), null)
  const reloaded = create()
  reloaded.sessionMeta = JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))
  const restored = await reloaded.getSideChat(parent.id)
  assert.equal(restored.session.id, created.session.id)
  assert.equal(restored.session.model, 'provider/nested/model')
  assert.equal(restored.created, false)
  assert.deepEqual(
    (await reloaded.listSessions()).map((item) => item.id),
    [parent.id],
  )
})

test('restarted expiration removes only temporary history and snapshots, preserving workspace files and usage totals', async (t) => {
  const { runtime, directory, create } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  const { session } = await runtime.getSideChat(parent.id, { create: true })
  const path = join(directory, 'user-output.txt')
  await writeFile(path, 'keep this file', 'utf8')
  const stored = await runtime.findSessionInfo(session.id)
  runtime.sessionMeta[session.id].sideChat.expiresAt = new Date(0).toISOString()
  await runtime.saveSessionMeta()
  const reloaded = create()
  reloaded.sessionMeta = JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))
  reloaded.usageLedger = { days: { '2026-09-27': { totalTokens: 123 } }, sessionScans: {} }
  await reloaded.sideChats.sweep()
  await assert.rejects(readFile(stored.path), { code: 'ENOENT' })
  assert.equal(await readFile(path, 'utf8'), 'keep this file')
  assert.equal(reloaded.usageLedger.days['2026-09-27'].totalTokens, 123)
  await assert.rejects(reloaded.getOrCreateSession(session.id), { code: 'side_chat_not_found' })
  assert.equal((await reloaded.listStoredSessions()).length, 1)
})

test('deleting a parent also removes its side chat without leaving a resident or pending child', async (t) => {
  const { runtime, directory } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  const { session } = await runtime.getSideChat(parent.id, { create: true })
  assert.equal(await runtime.deleteSession(parent.id), true)
  assert.equal(runtime.sessionMeta[session.id], undefined)
  assert.equal(runtime.pendingSessions.has(session.id), false)
  assert.equal((await runtime.listStoredSessions()).length, 0)
  await assert.rejects(runtime.getSideChat(parent.id, { create: true }), {
    code: 'session_not_found',
  })
})

test('HTTP side-chat creation ignores client overrides and expired chat/history requests return stable errors before SSE', async (t) => {
  const { runtime, directory } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  async function invoke(method, path, input = {}, id = parent.id) {
    const route = sessionRuntimeRoutes.find((item) => item.method === method && item.path === path)
    let result
    await route.handler({
      runtime,
      params: { sessionId: id },
      body: async () => input,
      json: (status, data) => {
        result = { status, data }
      },
      startSse: () => assert.fail('expired sessions must not start an SSE stream'),
    })
    return result
  }
  const first = await invoke('POST', '/api/sessions/:sessionId/side-chat', {
    cwd: '/untrusted',
    model: 'evil/model',
  })
  assert.equal(first.status, 200)
  assert.equal(first.data.session.cwd, directory)
  assert.equal(first.data.session.model, 'fallback/default')
  assert.equal(first.data.created, true)
  assert.equal((await invoke('POST', '/api/sessions/:sessionId/side-chat')).data.created, false)
  const childId = first.data.session.id
  runtime.sessionMeta[childId].sideChat.expiresAt = new Date(0).toISOString()
  for (const [method, path, input] of [
    ['GET', '/api/sessions/:sessionId/messages', {}],
    ['GET', '/api/sessions/:sessionId/live', {}],
    ['POST', '/api/chat', { sessionId: childId, message: 'hello' }],
  ]) {
    const response = await invoke(method, path, input, childId)
    assert.equal(response.status, 410)
    assert.equal(response.data.code, 'side_chat_expired')
  }
  await runtime.sideChats.sweep()
  assert.equal(
    (await invoke('POST', '/api/chat', { sessionId: childId, message: 'hello' }, childId)).data
      .code,
    'side_chat_not_found',
  )
})

test('failed prompts renew expiration in finally and leave ordinary session execution unchanged', async (t) => {
  const { runtime, directory } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  const { session } = await runtime.getSideChat(parent.id, { create: true })
  let now = Date.now()
  runtime.sideChats.now = () => now
  const value = { session: { sessionId: session.id }, cwd: directory }
  runtime.getOrCreateSession = async () => value
  runtime.installWorkspaceAssetCapture = () => {}
  runtime.touchSessionRuntime = () => {}
  runtime.evictIdleSessionRuntimes = () => {}
  runtime.runSessionPrompt = async () => {
    now += SIDE_CHAT_TTL_MS * 2
    await runtime.sideChats.sweep()
    assert.ok(runtime.sessionMeta[session.id])
    throw new Error('fixture failure')
  }
  await assert.rejects(runtime.streamPrompt({ sessionId: session.id }), /fixture failure/)
  assert.equal(
    Date.parse(runtime.sessionMeta[session.id].sideChat.expiresAt),
    now + SIDE_CHAT_TTL_MS,
  )
  assert.equal(runtime.sideChats.reservations.size, 0)
  assert.equal(value.runActive, false)
})

test('temporary answers stay on their SSE stream without automatic memory capture or external notifications', async (t) => {
  const { runtime, directory } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  const { session: summary } = await runtime.getSideChat(parent.id, { create: true })
  const observed = []
  const captured = []
  const events = []
  runtime.eventObserver = (event) => observed.push(event)
  runtime.captureConversationMemory = async (input) => {
    captured.push(input)
  }
  runtime.archiveAttachments = async () => []
  const session = {
    sessionId: summary.id,
    isStreaming: false,
    model: { provider: 'fixture', id: 'model' },
    thinkingLevel: 'medium',
    messages: [{ role: 'user', content: 'Earlier turn', timestamp: 1 }],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    subscribe: () => () => {},
    dispose: () => {},
    async prompt() {
      this.messages.push({
        role: 'assistant',
        content: [{ type: 'text', text: 'Side answer' }],
        timestamp: 2,
      })
    },
  }
  const value = {
    session,
    cwd: directory,
    name: 'Side chat',
    baseToolNames: [],
    enabledTools: ['memory_remember'],
  }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value
  await runtime.streamPrompt({
    sessionId: session.sessionId,
    message: 'A temporary question',
    send: (event, data) => events.push({ event, data }),
  })
  assert.equal(events.find((entry) => entry.event === 'done').data.text, 'Side answer')
  assert.deepEqual(captured, [])
  assert.deepEqual(observed, [])
  assert.equal(value.isolatedContext, undefined)
  assert.equal(value.blockedToolNames, undefined)
})

test('the last child-agent completion refreshes the temporary parent retention period', async (t) => {
  const { runtime, directory } = await runtimeFixture(t)
  const parent = await runtime.createSession('Parent', directory)
  const { session } = await runtime.getSideChat(parent.id, { create: true })
  const completedAt = Date.now() + SIDE_CHAT_TTL_MS * 2
  runtime.sideChats.now = () => completedAt
  runtime.emitAgentUpdate(session.id, { id: 'child-agent', status: 'completed' })
  await runtime.sessionMetaWrite
  assert.equal(
    Date.parse(runtime.sessionMeta[session.id].sideChat.expiresAt),
    completedAt + SIDE_CHAT_TTL_MS,
  )
})
