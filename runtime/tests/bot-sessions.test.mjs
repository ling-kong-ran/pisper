// Bot 就是一个长期会话加上它的设定：打开 Bot 打开这个会话，设定套到会话上，渠道与定时任务把话发进去，
// 回复与中途的审批、提问回到来源聊天。这里用最小的假 Runtime 驱动 AgentRuntimeFacade 上的 Bot 方法。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeFacade } from '../runtime/agent-runtime-facade.mjs'
import {
  botSystemPrompt,
  normalizeBotInput,
  normalizeBotMessage,
} from '../services/bots/bot-model.mjs'
import { botReplyText } from '../services/bots/bot-notifications.mjs'

function bot(id, overrides = {}) {
  return {
    id,
    ...normalizeBotInput({ name: id, instructions: '盯着首页性能', workspace: { cwd: '/repo' } }),
    threadSessionId: '',
    ...overrides,
  }
}

function deferred() {
  let resolve
  const promise = new Promise((ok) => {
    resolve = ok
  })
  return { promise, resolve }
}

// 只装 Bot 方法用得到的那几样，其余都不需要。
function fakeRuntime({ bots = [bot('小橙')], running = new Set(), prompt } = {}) {
  const registry = new Map(bots.map((item) => [item.id, item]))
  const calls = []
  const runtime = Object.create(AgentRuntimeFacade.prototype)
  Object.assign(runtime, {
    calls,
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => true,
    sessions: new Map(),
    pendingSessions: new Map(),
    startingRuns: new Map(),
    botChatOrigins: new Map(),
    bots: {
      list: () => [...registry.values()],
      get: (id) => (registry.has(id) ? { ...registry.get(id) } : null),
      create: async (input) => {
        const created = bot(input.name, { workspace: input.workspace })
        registry.set(created.id, created)
        return { ...created }
      },
      update: async (id, input) => {
        const updated = { ...registry.get(id), ...input }
        registry.set(id, updated)
        return { ...updated }
      },
      setThread: async (id, sessionId) => {
        registry.get(id).threadSessionId = sessionId
      },
    },
    findSessionInfo: async (id) => (id === 'existing' ? { path: '/sessions/existing' } : null),
    createSession: async (name, cwd) => {
      calls.push(['createSession', name, cwd])
      return { id: 'thread-1' }
    },
    executorSessionDirectory: async (environmentId) => `/data/executor-sessions/${environmentId}`,
    setSessionExecutor: async (id, executor) => calls.push(['executor', id, executor]),
    setSessionCwd: async (id, cwd) => calls.push(['cwd', id, cwd]),
    setSessionExecutionMode: async (id, mode) => calls.push(['mode', id, mode]),
    setSessionModel: async (id, provider, model) => calls.push(['model', id, provider, model]),
    renameSession: async (id, name) => calls.push(['rename', id, name]),
    disposeSessionRuntime: (id) => calls.push(['dispose', id]),
    providerPreferences: {
      resolveSubagentModel: async () => ({ model: { provider: 'luna', id: 'gpt-5.6-luna' } }),
    },
    sessionRunIsActive: (id) => running.has(id),
    queueSessionMessage: async (id, input) => calls.push(['queue', id, input]),
    promptFromChannel: async (input) => {
      calls.push(['prompt', input])
      return prompt ? prompt(input) : { sessionId: input.sessionId, text: '好的。', error: '' }
    },
    sent: [],
    channels: {
      sendToChat: async (platform, peerId, text) => {
        runtime.sent.push({ platform, peerId, text })
        return true
      },
    },
    permissions: { getPending: () => [] },
    questions: { getPending: () => [] },
    resolveToolApproval: (sessionId, approvalId, approved) => {
      calls.push(['resolve', sessionId, approvalId, approved])
      return { found: true }
    },
  })
  return runtime
}

const channel = { kind: 'channel', platform: 'feishu', peerId: 'chat-1', chatType: 'p2p' }

test('a new bot opens its own session with its folder, permission and model', async () => {
  const runtime = fakeRuntime({ bots: [] })
  const created = await runtime.createBot({ name: '巡检', workspace: { cwd: '/repo' } })
  assert.equal(created.threadSessionId, 'thread-1')
  assert.deepEqual(runtime.calls.slice(0, 5), [
    ['createSession', '巡检', '/repo'],
    ['executor', 'thread-1', null],
    ['cwd', 'thread-1', '/repo'],
    ['mode', 'thread-1', 'approval-required'],
    ['model', 'thread-1', 'luna', 'gpt-5.6-luna'],
  ])
  // 指令进系统提示：释放空闲的运行时，下次用到时按新设定重建。
  assert.deepEqual(runtime.calls.at(-1), ['dispose', 'thread-1'])
  // 再打开时沿用同一个会话，不另建。
  runtime.findSessionInfo = async () => ({ path: '/sessions/thread-1' })
  assert.equal(await runtime.ensureBotSession(created.id), 'thread-1')
  assert.equal(runtime.calls.filter(([name]) => name === 'createSession').length, 1)
})

test('a bot on another computer runs its session there, with a placeholder folder here', async () => {
  const remote = bot('远程', {
    workspace: { environmentId: 'env_testmachine1', cwd: '/work/site' },
  })
  const runtime = fakeRuntime({ bots: [remote] })
  assert.equal(await runtime.ensureBotSession('远程'), 'thread-1')
  assert.deepEqual(runtime.calls.slice(0, 3), [
    ['createSession', '远程', '/data/executor-sessions/env_testmachine1'],
    ['executor', 'thread-1', { environmentId: 'env_testmachine1', cwd: '/work/site' }],
    ['cwd', 'thread-1', '/data/executor-sessions/env_testmachine1'],
  ])
})

test('changing a bot applies to its session and renaming it renames the session', async () => {
  const runtime = fakeRuntime({ bots: [bot('小橙', { threadSessionId: 'existing' })] })
  await runtime.updateBot('小橙', { name: '小橘', executionMode: 'workspace-write' })
  assert.ok(runtime.calls.some((call) => call[0] === 'mode' && call[2] === 'workspace-write'))
  assert.deepEqual(runtime.calls.at(-1), ['rename', 'existing', '小橘'])
  runtime.calls.length = 0
  await runtime.updateBot('小橙', { instructions: '只看不改' })
  assert.equal(runtime.calls.filter(([name]) => name === 'rename').length, 0)
  assert.ok(runtime.calls.some((call) => call[0] === 'dispose'))
})

test('a chat message goes into the bot session and the reply returns to that chat', async () => {
  const turn = deferred()
  const runtime = fakeRuntime({
    bots: [bot('小橙', { threadSessionId: 'existing' })],
    prompt: async () => {
      await turn.promise
      return { sessionId: 'existing', text: '下一次在今天 18:00。', error: '' }
    },
  })
  const sent = await runtime.messageBot('小橙', {
    text: '  看看我的定时任务还有多久执行  ',
    source: channel,
  })
  assert.deepEqual(sent, { sessionId: 'existing', status: 'started' })
  const [, prompt] = runtime.calls.find(([name]) => name === 'prompt')
  assert.equal(prompt.sessionId, 'existing')
  assert.equal(prompt.message, '看看我的定时任务还有多久执行')
  assert.equal(prompt.unattended, false)
  // 这一轮从哪个聊天来：审批与提问据此回到那里，说完就清掉。
  assert.equal(runtime.botChatOrigins.get('existing'), channel)
  turn.resolve()
  await new Promise((resolve) => setTimeout(resolve, 0))
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.deepEqual(runtime.sent, [
    { platform: 'feishu', peerId: 'chat-1', text: '下一次在今天 18:00。' },
  ])
  assert.equal(runtime.botChatOrigins.has('existing'), false)
})

test('a busy bot tells the chat it is busy, while a schedule queues behind the current turn', async () => {
  const runtime = fakeRuntime({
    bots: [bot('小橙', { threadSessionId: 'existing' })],
    running: new Set(['existing']),
  })
  assert.deepEqual(await runtime.messageBot('小橙', { text: '在吗', source: channel }), {
    sessionId: 'existing',
    status: 'busy',
  })
  assert.deepEqual(
    await runtime.messageBot('小橙', {
      text: '做今天的依赖巡检',
      source: { kind: 'schedule', scheduleTaskId: 'sched-1' },
    }),
    { sessionId: 'existing', status: 'queued' },
  )
  assert.deepEqual(runtime.calls.at(-1), [
    'queue',
    'existing',
    { message: '做今天的依赖巡检', behavior: 'followUp' },
  ])
  assert.equal(runtime.calls.filter(([name]) => name === 'prompt').length, 0)
})

test('paused bots and empty messages are refused; failures reach the chat', async () => {
  const runtime = fakeRuntime({
    bots: [bot('停着', { enabled: false }), bot('小橙', { threadSessionId: 'existing' })],
    prompt: async () => ({ sessionId: 'existing', text: '', error: '模型不可用' }),
  })
  await assert.rejects(runtime.messageBot('停着', { text: 'x' }), { code: 'bot_paused' })
  await assert.rejects(runtime.messageBot('小橙', { text: '   ' }), { code: 'bot_message_empty' })
  await assert.rejects(runtime.messageBot('没有', { text: 'x' }), { code: 'bot_not_found' })
  await runtime.messageBot('小橙', { text: '跑一下', source: channel })
  await new Promise((resolve) => setTimeout(resolve, 0))
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.equal(runtime.sent[0].text, '小橙 没做成：模型不可用')
})

test('activity reads the session: idle, running, or waiting for you', () => {
  const thread = bot('小橙', { threadSessionId: 'existing' })
  const running = new Set()
  const runtime = fakeRuntime({ bots: [thread], running })
  assert.equal(runtime.botActivity(bot('新的')), null)
  assert.deepEqual(runtime.botActivity(thread), { status: 'idle', waiting: null, needsYou: false })
  running.add('existing')
  assert.deepEqual(runtime.botActivity(thread), {
    status: 'running',
    waiting: null,
    needsYou: false,
  })
  runtime.permissions.getPending = () => [
    {
      id: 'a1',
      toolName: 'bash',
      args: { command: 'npm test' },
      createdAt: '1',
      severity: 'standard',
    },
  ]
  assert.deepEqual(runtime.botActivity(thread), {
    status: 'running',
    waiting: 'approval',
    needsYou: true,
  })
})

test('approvals in a chat-started turn reach that chat and can be answered there', async () => {
  const runtime = fakeRuntime({ bots: [bot('小橙', { threadSessionId: 'existing' })] })
  const approval = {
    id: 'a1',
    toolName: 'bash',
    args: { command: 'npm test' },
    severity: 'standard',
  }
  // 没有来源聊天（在应用里说的话）时不往任何聊天发。
  runtime.botSessionEvent('existing', 'permission_request', approval)
  assert.equal(runtime.sent.length, 0)
  runtime.botChatOrigins.set('existing', channel)
  runtime.botSessionEvent('existing', 'permission_request', approval)
  await new Promise((resolve) => setTimeout(resolve, 0))
  assert.match(runtime.sent[0].text, /小橙 需要批准：bash[\s\S]*回复 \/小橙 批准 或 \/小橙 拒绝/)
  // 聊天里的“批准”处理的是 Bot 会话里最早一条待审批。
  runtime.permissions.getPending = () => [{ ...approval, createdAt: '1' }]
  assert.equal(await runtime.botChannelAction('小橙', 'approve'), '已批准，小橙 继续干活。')
  assert.deepEqual(runtime.calls.at(-1), ['resolve', 'existing', 'a1', true])
  runtime.permissions.getPending = () => []
  assert.equal(await runtime.botChannelAction('小橙', 'deny'), '小橙 现在没有等批准的操作。')
})

test('the bot system prompt says who it is and carries its instructions', () => {
  assert.match(botSystemPrompt(bot('小橙')), /你是 小橙[\s\S]*\[小橙 的工作说明\]\n盯着首页性能/)
  assert.doesNotMatch(botSystemPrompt(bot('空白', { instructions: '' })), /工作说明/)
  assert.equal(normalizeBotMessage('  hi  '), 'hi')
  assert.throws(() => normalizeBotMessage('x'.repeat(12_001)), { code: 'bot_message_too_long' })
  assert.equal(botReplyText('小橙', { text: '' }), '小橙 做完了。')
  assert.match(botReplyText('小橙', { text: '长'.repeat(3_100) }), /完整回复在 Pisper 里/)
})

test('every turn in a bot session runs with the bot scope, prompt and isolation', async () => {
  // 不管这句话来自应用、聊天还是定时任务，运行时都按会话找到 Bot 并套用它的设定。
  const source = await readFile(new URL('../runtime/agent-runtime.mjs', import.meta.url), 'utf8')
  assert.match(
    source,
    /const bot = this\.botForSession\?\.\(value\.session\?\.sessionId \|\| sessionId\)\s*if \(bot\) \{\s*allowedToolNames = bot\.tools\s*isolatedContext = true\s*\}/,
  )
  assert.match(source, /bot \? \{ appendSystemPrompt: botSystemPrompt\(bot\) \} : undefined/)
  // 关掉 Bots 实验功能后不再套用。
  const runtime = fakeRuntime({ bots: [bot('小橙', { threadSessionId: 'existing' })] })
  assert.equal(runtime.botForSession('existing')?.id, '小橙')
  runtime.isBetaFeatureEnabled = () => false
  assert.equal(runtime.botForSession('existing'), null)
})

test('the session route opens a bot behind the Labs gate', async () => {
  let enabled = false
  const runtime = {
    capabilities: { features: { bots: true } },
    isBetaFeatureEnabled: () => enabled,
    ensureBotSession: async (id) => `thread-of-${id}`,
  }
  const handler = createApiHandler(runtime)
  const respond = async (path) => {
    const output = {
      status: 0,
      body: '',
      destroyed: false,
      writableEnded: false,
      headersSent: false,
      writeHead(status) {
        this.status = status
      },
      flushHeaders() {},
      write(chunk = '') {
        this.body += chunk
      },
      end(chunk = '') {
        this.body += chunk
        this.writableEnded = true
      },
    }
    await handler(
      { method: 'POST', headers: {}, async *[Symbol.asyncIterator]() {} },
      output,
      new URL(`http://localhost${path}`),
    )
    return { status: output.status, body: JSON.parse(output.body) }
  }
  assert.equal((await respond('/api/bots/bot_1/session')).body.code, 'beta_feature_disabled')
  enabled = true
  assert.deepEqual(await respond('/api/bots/bot_1/session'), {
    status: 200,
    body: { sessionId: 'thread-of-bot_1' },
  })
  // 任务那一层没有了：旧的任务接口不再存在。
  assert.equal((await respond('/api/bot-tasks/task_1/accept')).status, 404)
})
