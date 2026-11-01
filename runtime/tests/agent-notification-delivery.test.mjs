// 子 Agent 完成通知的投递：steer 失败时不静默丢失，改随本条消息发出或在本轮结束后补发。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

async function setup(t, { steer, prompt = async () => {} }) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-agent-note-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.archiveAttachments = async () => []
  runtime.captureConversationMemory = async () => []
  runtime.memory = { relevantContext: async () => ({ text: '' }) }
  const session = {
    sessionId: 'note-session',
    model: { provider: 'openai', id: 'gpt-5' },
    thinkingLevel: 'medium',
    isStreaming: false,
    messages: [],
    agent: { state: { systemPrompt: '' } },
    getActiveToolNames: () => [],
    setActiveToolsByName: () => {},
    setSessionName: () => {},
    subscribe: () => () => {},
    steer,
    prompt: (text) => prompt(text, runtime, value),
    async abort() {},
    dispose() {},
  }
  const value = { session, cwd: directory, name: '通知', baseToolNames: [] }
  runtime.sessions.set(session.sessionId, value)
  runtime.getOrCreateSession = async () => value
  t.after(() => {
    for (const timer of runtime.agentWakeupTimers.values()) clearTimeout(timer)
  })
  return { runtime, value }
}

test('a pending agent note that cannot be steered goes out with the prompt itself', async (t) => {
  let prompted = ''
  const { runtime, value } = await setup(t, {
    steer: async () => {
      throw new Error('queue rejected')
    },
    prompt: async (text) => {
      prompted = text
    },
  })
  value.pendingAgentNotifications = ['[agent done] reviewer finished']
  await runtime.streamPrompt({ sessionId: 'note-session', message: '继续', send: () => {} })
  assert.match(prompted, /^继续[\s\S]*\[agent done\] reviewer finished/)
  assert.deepEqual(value.pendingAgentNotifications, [])
})

test('a note that fails to steer into a running session is retried after the run', async (t) => {
  const warn = t.mock.method(console, 'warn', () => {})
  const { runtime, value } = await setup(t, {
    steer: async () => {
      throw new Error('queue rejected')
    },
    prompt: async (_text, current, resident) => {
      await current.steerAgentNotification(resident, 'note A')
    },
  })
  await runtime.streamPrompt({ sessionId: 'note-session', message: '跑一轮', send: () => {} })
  assert.deepEqual(value.pendingAgentNotifications, ['note A'])
  assert.equal(runtime.agentWakeupTimers.has('note-session'), true)
  // 日志只说明发生了什么，不带通知内容。
  assert.doesNotMatch(String(warn.mock.calls[0].arguments[0]), /note A/)
})

test('an explicit stop does not schedule a wakeup for undelivered notes', async (t) => {
  t.mock.method(console, 'warn', () => {})
  const { runtime, value } = await setup(t, {
    steer: async () => {
      throw new Error('queue rejected')
    },
    prompt: async (_text, current, resident) => {
      await current.steerAgentNotification(resident, 'note B')
      resident.abortedAt = Date.now()
    },
  })
  await runtime.streamPrompt({ sessionId: 'note-session', message: '跑一轮', send: () => {} })
  assert.deepEqual(value.pendingAgentNotifications, ['note B'])
  assert.equal(runtime.agentWakeupTimers.has('note-session'), false)
})
