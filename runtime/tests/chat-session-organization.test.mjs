import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { sessionRuntimeRoutes } from '../http/routes/sessions-runtime.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  normalizeSessionOrganization,
  projectSessionOrganization,
} from '../services/chat-session-organization.mjs'

const organizationRoute = sessionRuntimeRoutes.find(
  (route) => route.method === 'PATCH' && route.path === '/api/sessions/:sessionId/organization',
)
const listRoute = sessionRuntimeRoutes.find(
  (route) => route.method === 'GET' && route.path === '/api/sessions',
)

function freshRuntime(directory) {
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  return runtime
}

async function patch(runtime, id, input) {
  let response
  await organizationRoute.handler({
    runtime,
    params: { sessionId: id },
    body: async () => input,
    json: (status, value) => {
      response = { status, value }
    },
  })
  return response
}

test('session organization is additive, durable, and retained through archive and restore', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-organization-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)
  const created = await runtime.createSession('组织测试', directory)
  assert.equal(
    (await runtime.getFileChangesService().summary(created.id, directory)).status,
    'known',
  )
  assert.deepEqual(
    {
      pinned: created.pinned,
      archived: created.archived,
      unread: created.unread,
      needsAttention: created.needsAttention,
      attentionReason: created.attentionReason,
      lastCompletedAt: created.lastCompletedAt,
    },
    {
      pinned: false,
      archived: false,
      unread: false,
      needsAttention: false,
      attentionReason: null,
      lastCompletedAt: null,
    },
  )

  const before = (await runtime.listSessions()).find((session) => session.id === created.id)
  const [pinned, archived] = await Promise.all([
    patch(runtime, created.id, { pinned: true }),
    patch(runtime, created.id, { archived: true }),
  ])
  assert.equal(pinned.status, 200)
  assert.equal(archived.status, 200)
  assert.equal(archived.value.id, created.id)
  assert.equal(archived.value.name, '组织测试')
  assert.equal(archived.value.pinned, true)
  assert.equal(archived.value.archived, true)
  assert.equal(archived.value.modified, before.modified)
  assert.equal(
    (await runtime.listSessions()).find((session) => session.id === created.id).archived,
    true,
  )
  let catalog
  await listRoute.handler({
    runtime,
    json: (status, value) => {
      assert.equal(status, 200)
      catalog = value
    },
  })
  assert.equal(catalog.sessions.find((session) => session.id === created.id).archived, true)

  const persisted = JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))
  assert.equal(persisted[created.id].organization.pinned, true)
  assert.equal(persisted[created.id].organization.archived, true)
  const reloaded = freshRuntime(directory)
  reloaded.sessionMeta = persisted
  const restored = (await reloaded.listSessions()).find((session) => session.id === created.id)
  assert.equal(restored.pinned, true)
  assert.equal(restored.archived, true)
  assert.equal((await patch(reloaded, created.id, { archived: false })).value.archived, false)
  assert.equal(await reloaded.deleteSession(created.id), true)
  await reloaded.sessionLifecycle.recordSessionCompletion(created.id, false)
  assert.equal(
    (await reloaded.listSessions()).some((session) => session.id === created.id),
    false,
  )
  assert.equal(JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))[created.id], undefined)
})

test('organization PATCH rejects empty, unknown, and non-boolean fields without mutation', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-organization-invalid-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)
  const created = await runtime.createSession('合法会话', directory)
  for (const input of [null, [], {}, { pinned: 'true' }, { archived: 1 }, { unknown: true }]) {
    const response = await patch(runtime, created.id, input)
    assert.equal(response.status, 400)
    assert.equal(response.value.code, 'invalid_session_organization')
  }
  assert.equal((await patch(runtime, 'missing-session', { pinned: true })).status, 404)
  assert.equal(runtime.sessionMeta[created.id].organization, undefined)
  assert.equal(
    (await runtime.listSessions()).find((session) => session.id === created.id).pinned,
    false,
  )
})

test('new chats remain organizable while pending materialization changes ownership', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-organization-transition-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)
  const created = await runtime.createSession('并发整理', directory)
  const pending = runtime.pendingSessions.get(created.id)
  assert.ok(pending)
  const initialFile = await readFile(pending.manager.getSessionFile(), 'utf8')
  assert.match(initialFile, /并发整理/)

  // 首轮消息接管 pending 后，文件和持久目录仍是同一会话的事实来源。
  runtime.pendingSessions.delete(created.id)
  const response = await patch(runtime, created.id, { pinned: true })
  assert.equal(response.status, 200)
  assert.equal(response.value.pinned, true)
  assert.equal(await readFile(pending.manager.getSessionFile(), 'utf8'), initialFile)
})

test('unread tracks completed runs and explicit reads while attention follows real failure or approval', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-organization-state-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)
  const created = await runtime.createSession('状态测试', directory)
  const id = created.id

  await runtime.renameSession(id, '状态测试已重命名')
  assert.equal((await runtime.listSessions()).find((session) => session.id === id).unread, false)
  assert.equal((await patch(runtime, id, { read: false })).value.unread, true)
  assert.equal((await patch(runtime, id, { read: true })).value.unread, false)

  await runtime.sessionLifecycle.recordSessionCompletion(id, true, '2026-09-24T00:00:00.000Z')
  let summary = (await runtime.listSessions()).find((session) => session.id === id)
  assert.equal(summary.unread, true)
  assert.equal(summary.needsAttention, true)
  assert.equal(summary.attentionReason, 'failure')
  assert.equal(summary.lastCompletedAt, '2026-09-24T00:00:00.000Z')
  summary = (await patch(runtime, id, { read: true })).value
  assert.equal(summary.unread, false)
  assert.equal(summary.needsAttention, true)
  const failedReload = freshRuntime(directory)
  failedReload.sessionMeta = JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))
  assert.equal(
    (await failedReload.listSessions()).find((session) => session.id === id).attentionReason,
    'failure',
  )

  runtime.permissions.pending.set('approval-1', { id: 'approval-1', sessionId: id })
  assert.equal(
    (await runtime.listSessions()).find((session) => session.id === id).attentionReason,
    'approval',
  )
  runtime.permissions.pending.delete('approval-1')
  assert.equal(
    (await runtime.listSessions()).find((session) => session.id === id).attentionReason,
    'failure',
  )

  await runtime.sessionLifecycle.recordSessionCompletion(id, false, '2026-09-24T00:01:00.000Z')
  summary = (await runtime.listSessions()).find((session) => session.id === id)
  assert.equal(summary.unread, true)
  assert.equal(summary.needsAttention, false)
  assert.equal(summary.attentionReason, null)

  const reloaded = freshRuntime(directory)
  reloaded.sessionMeta = JSON.parse(await readFile(runtime.sessionMetaPath, 'utf8'))
  const persistedSummary = (await reloaded.listSessions()).find((session) => session.id === id)
  assert.equal(persistedSummary.unread, true)
  assert.equal(persistedSummary.needsAttention, false)
  assert.equal(persistedSummary.lastCompletedAt, '2026-09-24T00:01:00.000Z')
})

test('old or malformed organization metadata projects safe defaults without changing other metadata', () => {
  assert.deepEqual(normalizeSessionOrganization({ name: 'legacy' }), {
    pinned: false,
    archived: false,
    unread: false,
    failed: false,
    lastCompletedAt: null,
  })
  assert.deepEqual(projectSessionOrganization({ organization: { pinned: 'yes', failed: 1 } }), {
    pinned: false,
    archived: false,
    unread: false,
    needsAttention: false,
    attentionReason: null,
    lastCompletedAt: null,
  })
})

test('file change marker failure leaves new sessions usable and logs no private details', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-marker-failure-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)
  runtime.sessionLifecycle.markSessionTracked = async () => {
    throw new Error('private path and token')
  }
  const warnings = []
  const originalWarn = console.warn
  console.warn = (...args) => warnings.push(args.join(' '))
  t.after(() => {
    console.warn = originalWarn
  })

  const created = await runtime.createSession('仍然可用', directory)
  assert.equal(
    (await runtime.listSessions()).find((session) => session.id === created.id).name,
    '仍然可用',
  )
  assert.equal(warnings.length, 1)
  assert.doesNotMatch(warnings[0], /private|token|pisper-session-marker-failure/)
})
