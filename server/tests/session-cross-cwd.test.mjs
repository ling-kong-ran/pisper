import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { SessionManager } from '@earendil-works/pi-coding-agent'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

function assistantMessage(text, timestamp) {
  return {
    role: 'assistant',
    content: [{ type: 'text', text }],
    api: 'openai-responses',
    provider: 'openai',
    model: 'gpt-test',
    usage: {
      input: 0,
      output: 0,
      cacheRead: 0,
      cacheWrite: 0,
      totalTokens: 0,
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
    },
    stopReason: 'stop',
    timestamp,
  }
}

function createPersistedSession(cwd, sessionDir, prompt) {
  const manager = SessionManager.create(cwd, sessionDir)
  const timestamp = Date.now()
  manager.appendModelChange('openai', 'gpt-test')
  manager.appendMessage({ role: 'user', content: prompt, timestamp })
  manager.appendMessage(assistantMessage(`Reply to ${prompt}`, timestamp + 1))
  return manager
}

test('desktop runtime lists and opens sessions created under other working directories', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-cross-cwd-'))
  const dataDir = join(directory, 'agent')
  const sessionDir = join(dataDir, 'sessions')
  const desktopCwd = join(directory, 'desktop-home')
  const webCwd = join(directory, 'web-project')
  await Promise.all([
    mkdir(sessionDir, { recursive: true }),
    mkdir(desktopCwd, { recursive: true }),
    mkdir(webCwd, { recursive: true }),
  ])

  const webSession = createPersistedSession(webCwd, sessionDir, 'Web session')
  const desktopSession = createPersistedSession(desktopCwd, sessionDir, 'Desktop session')
  const runtime = new AgentRuntimeService({ cwd: desktopCwd, dataDir })
  t.after(async () => {
    runtime.sessions.clear()
    await runtime.dispose().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  runtime.goals = { get: () => null }
  runtime.plans = { get: () => null }
  runtime.multiAgents = { summaries: () => [] }

  const sessions = await runtime.listSessions()
  assert.deepEqual(
    new Set(sessions.map((session) => session.id)),
    new Set([webSession.getSessionId(), desktopSession.getSessionId()]),
  )
  assert.equal(
    sessions.find((session) => session.id === webSession.getSessionId())?.cwd,
    resolve(webCwd),
  )
  assert.equal(sessions[0].plan, null)
  assert.equal(Object.hasOwn(sessions[0], 'taskList'), false)

  const info = await runtime.findSessionInfo(webSession.getSessionId())
  assert.equal(info?.cwd, resolve(webCwd))
  assert.equal(runtime.openStoredSession(info.path).getCwd(), resolve(webCwd))

  const messages = await runtime.getSessionMessages(webSession.getSessionId())
  assert.equal(messages[0]?.text, 'Web session')

  runtime.skills = { dashboard: async ({ cwd }) => ({ cwd }) }
  const skills = await runtime.getSkillsDashboard(webSession.getSessionId())
  assert.equal(skills.cwd, resolve(webCwd))

  runtime.getSessionMessagePage = async () => ({ messages: [], contextUsage: null, pageInfo: {} })
  runtime.permissions = { getPending: () => [] }
  const live = await runtime.getSessionLive(webSession.getSessionId())
  assert.equal(live.cwd, resolve(webCwd))
  assert.equal(live.plan, null)
  assert.equal(Object.hasOwn(live, 'taskList'), false)

  runtime.createSessionRuntime = async (manager) => ({ cwd: manager.getCwd(), manager })
  const opened = await runtime.getOrCreateSession(webSession.getSessionId())
  assert.equal(opened.cwd, resolve(webCwd))

  // Cold history may schedule deferred session-meta persistence; wait before teardown.
  await runtime.sessionMetaWrite.catch(() => {})
})
