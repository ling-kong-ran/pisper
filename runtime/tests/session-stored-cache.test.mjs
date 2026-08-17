import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { ensureSessionFilePersisted } from '../runtime/session-file-persist.mjs'

function freshRuntime(directory) {
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.settingsManager = {
    getGlobalSettings: () => ({
      defaultProvider: 'openai',
      defaultModel: 'gpt-5.4',
      defaultThinkingLevel: 'medium',
    }),
  }
  return runtime
}

test('pre-persisted sessions remain writable when the first assistant message arrives', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-first-assistant-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)

  const created = await runtime.createSession('首条回复', directory)
  const pending = runtime.sessionLifecycle.pendingSessions.get(created.id)
  await ensureSessionFilePersisted(pending.manager, '首条回复', directory)

  pending.manager.appendMessage({
    role: 'assistant',
    content: [{ type: 'text', text: '完成' }],
    timestamp: Date.now(),
  })

  const entries = (await readFile(pending.manager.sessionFile, 'utf8'))
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line))
  assert.equal(entries.filter((entry) => entry.type === 'session').length, 1)
  assert.equal(entries.at(-1).message.role, 'assistant')
})

test('a fresh session stays addressable after resident eviction and workspace switch', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-stored-cache-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)

  // Prime the stored-session cache BEFORE the conversation exists, the way a
  // long-lived runtime keeps a snapshot from before a panel opened.
  await runtime.listStoredSessions()

  // Create the session and materialize it (the runtime writes the minimal
  // session file at materialization even before the first assistant reply).
  const created = await runtime.createSession('缓存会话', directory)
  assert.ok(created.id)
  const pending = runtime.sessionLifecycle.pendingSessions.get(created.id)
  assert.ok(pending, 'the fresh session should be pending before materialization')
  await ensureSessionFilePersisted(pending.manager, '缓存会话', directory)
  await runtime.listStoredSessions({ refresh: true })

  // The materialized conversation must be listed.
  const sessions = await runtime.listSessions()
  assert.ok(
    sessions.some((item) => item.id === created.id),
    'listSessions must include the materialized session',
  )

  // Simulate the reported bug: the resident runtime was released (forced
  // interruption / idle sweep) and the stored-session cache is stale from
  // before the session file existed.
  runtime.sessions.clear()
  runtime.pendingSessions.delete(created.id)
  runtime.storedSessionsCache = []
  runtime.storedSessionsPromise = null

  // findSessionInfo must fall back to a disk scan instead of reporting the
  // session as missing (the root of the "session not found" workspace error).
  const info = await runtime.findSessionInfo(created.id)
  assert.ok(info, 'findSessionInfo must fall back to a disk scan')
  assert.equal(info.id, created.id)
})

test('createSession updates the stored-session cache incrementally without a rescan', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-incremental-cache-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = freshRuntime(directory)

  // 预热缓存，模拟长驻进程的首次快照；包装 listStoredSessions 统计 refresh。
  await runtime.listStoredSessions()
  let rescans = 0
  const originalListStored = runtime.listStoredSessions.bind(runtime)
  runtime.listStoredSessions = (options) => {
    if (options?.refresh) rescans += 1
    return originalListStored(options)
  }
  runtime.createSessionRuntime = async (manager, name) => {
    const now = new Date().toISOString()
    return {
      manager,
      name,
      session: { messages: [], sessionFile: manager.getSessionFile() },
      cwd: directory,
      created: now,
      modified: now,
    }
  }

  const created = await runtime.createSession('增量缓存会话', directory)
  // 新建会话不得触发全量重扫。
  assert.equal(rescans, 0)
  const sessions = await runtime.listSessions()
  const listed = sessions.find((item) => item.id === created.id)
  assert.ok(listed, 'newly created session must be visible without a rescan')
  assert.equal(listed.name, '增量缓存会话')
  assert.equal(listed.messageCount, 0)

  // 物化（模拟第一次 prompt 写盘）后同样无需重扫，且仍可被列出。
  const pending = runtime.sessionLifecycle.pendingSessions.get(created.id)
  await ensureSessionFilePersisted(pending.manager, '增量缓存会话', directory)
  await runtime.getOrCreateSession(created.id)
  assert.equal(rescans, 0)
  const after = await runtime.listSessions()
  assert.ok(
    after.some((item) => item.id === created.id),
    'materialized session stays listed',
  )
})

test('createSession persists the workspace so idle eviction does not reset cwd', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-stored-cwd-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const workspace = join(directory, 'workspace')
  await mkdir(workspace, { recursive: true })
  const runtime = freshRuntime(directory)

  const created = await runtime.createSession('工作目录会话', workspace)
  assert.equal(created.cwd, workspace)
  const pending = runtime.sessionLifecycle.pendingSessions.get(created.id)
  await ensureSessionFilePersisted(pending.manager, '工作目录会话', workspace)
  await runtime.listStoredSessions({ refresh: true })

  // After the resident runtime is released, listSessions must still report
  // the chosen workspace (from sessionMeta), not the launch default.
  runtime.sessions.clear()
  runtime.pendingSessions.delete(created.id)
  const sessions = await runtime.listSessions()
  const listed = sessions.find((item) => item.id === created.id)
  assert.ok(listed, 'the evicted session must remain listed')
  assert.equal(listed.cwd, workspace, 'cwd must survive resident eviction')
})
