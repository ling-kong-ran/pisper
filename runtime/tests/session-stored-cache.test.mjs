import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService, ensureSessionFilePersisted } from '../runtime/agent-runtime.mjs'

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
