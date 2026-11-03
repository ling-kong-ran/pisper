import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { createAgentSession, SessionManager, SettingsManager } from '../runtime/pi-coding-agent.mjs'
import { promoteQueuedSessionInput } from '../runtime/session-input-queue.mjs'
import { serializeTranscriptMessages } from '../runtime/stream-projection.mjs'
import { installStepInterrupt, interruptCurrentStep } from '../runtime/step-interrupt.mjs'

const { createExtensionRuntime } = await import(
  new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent'))
    .href
)

const MODEL = {
  id: 'interrupt-fixture',
  provider: 'fixture',
  api: 'openai-completions',
  name: 'Interrupt fixture',
  input: ['text'],
  reasoning: false,
  contextWindow: 128000,
  maxTokens: 4096,
}

async function realSession(t) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-step-interrupt-'))
  const extensions = { extensions: [], errors: [], runtime: createExtensionRuntime() }
  const { session } = await createAgentSession({
    cwd: directory,
    agentDir: directory,
    model: MODEL,
    modelRuntime: {},
    tools: [],
    sessionManager: SessionManager.inMemory(directory),
    settingsManager: SettingsManager.inMemory({ compaction: { enabled: false } }),
    resourceLoader: {
      getExtensions: () => extensions,
      getSkills: () => ({ skills: [], diagnostics: [] }),
      getPrompts: () => ({ prompts: [], diagnostics: [] }),
      getThemes: () => ({ themes: [], diagnostics: [] }),
      getAgentsFiles: () => ({ agentsFiles: [] }),
      getSystemPrompt: () => 'Interrupt fixture',
      getAppendSystemPrompt: () => [],
      extendResources() {},
      async reload() {},
    },
  })
  t.after(async () => {
    session.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  return { session, directory }
}

// 模拟模型流：按 pieces 逐段吐字，收到中止信号时像 pi-ai 一样以 aborted 结束；result() 给出最终消息。
function scriptedStream(pieces, { delayMs = 20, onCall } = {}) {
  let calls = 0
  return async (model, context, options) => {
    const call = calls++
    onCall?.(call, context)
    const signal = options?.signal
    const parts = pieces[Math.min(call, pieces.length - 1)]
    const base = {
      role: 'assistant',
      api: model.api,
      provider: model.provider,
      model: model.id,
      usage: {
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
      timestamp: Date.now(),
    }
    let final = null
    return {
      async *[Symbol.asyncIterator]() {
        let text = ''
        const partial = () => ({ ...base, content: [{ type: 'text', text }], stopReason: 'stop' })
        yield { type: 'start', partial: partial() }
        for (const piece of parts) {
          await new Promise((resolve) => setTimeout(resolve, delayMs))
          if (signal?.aborted) {
            final = { ...partial(), stopReason: 'aborted', errorMessage: 'Request was aborted' }
            yield { type: 'error', reason: 'aborted', error: final }
            return
          }
          text += piece
          yield { type: 'text_delta', contentIndex: 0, delta: piece, partial: partial() }
        }
        final = partial()
        yield { type: 'done', reason: 'stop', message: final }
      },
      async result() {
        return final
      },
    }
  }
}

// Pi 会在回合边界插入声明工具变化的 system 消息，这里只看对话本身。
const conversation = (messages) => messages.filter((message) => message.role !== 'system')

const textOf = (message) =>
  (Array.isArray(message.content) ? message.content : [{ type: 'text', text: message.content }])
    .filter((part) => part.type === 'text')
    .map((part) => part.text)
    .join('')

test('send now cuts the streaming reply short and continues the same run with the steering message', async (t) => {
  const { session } = await realSession(t)
  const contexts = []
  session.agent.streamFunction = scriptedStream(
    [['第一段。', '第二段。', '第三段。', '第四段。', '第五段。'], ['按插队的要求重写。']],
    { onCall: (call, context) => contexts.push(context.messages.map((m) => m.role)) },
  )
  const original = session.agent.streamFunction
  installStepInterrupt(session)
  let agentEnds = 0
  session.agent.subscribe((event) => {
    if (event.type === 'agent_end') agentEnds += 1
  })
  const run = session.agent.prompt('写一段很长的说明')
  // 第二段写出后插队：先按 steer 入队，再打断当前这一步。
  await new Promise((resolve) => setTimeout(resolve, 55))
  session.agent.steer({
    role: 'user',
    content: [{ type: 'text', text: '改成三条要点' }],
    timestamp: Date.now(),
  })
  assert.equal(interruptCurrentStep(session), true)
  await run
  // 换上的可打断流函数只在运行开始时交给循环，之后立刻换回原函数（压缩摘要靠它的身份取凭据）。
  assert.equal(session.agent.streamFunction, original)
  assert.equal(agentEnds, 1, 'one run, not an aborted run plus a new one')
  const messages = conversation(session.agent.state.messages)
  assert.deepEqual(
    messages.map((message) => message.role),
    ['user', 'assistant', 'user', 'assistant'],
  )
  assert.equal(messages[1].stopReason, 'stop', 'the interrupted reply is kept as a normal reply')
  assert.equal(messages[1].pisperInterrupted, true, 'the marker is persisted with the message')
  assert.equal(messages[3].pisperInterrupted, undefined)
  // 历史投影：被打断的那段标 interrupted，插队消息和后面的回复照常。
  assert.deepEqual(
    serializeTranscriptMessages(session.agent.state.messages).map((item) => [
      item.role,
      Boolean(item.interrupted),
    ]),
    [
      ['user', false],
      ['agent', true],
      ['user', false],
      ['agent', false],
    ],
  )
  assert.match(textOf(messages[1]), /^第一段。第二段。/)
  assert.doesNotMatch(textOf(messages[1]), /第五段/)
  assert.equal(textOf(messages[2]), '改成三条要点')
  assert.equal(textOf(messages[3]), '按插队的要求重写。')
  // 第二次请求的上下文里带着被打断的回复与插队消息。
  assert.deepEqual(
    contexts[1].filter((role) => role !== 'system'),
    ['user', 'assistant', 'user'],
  )
  assert.equal(interruptCurrentStep(session), false, 'nothing to interrupt once the run settles')
})

test('an interrupted reply keeps only its written text and drops half-written tool calls and thinking', async (t) => {
  const { session } = await realSession(t)
  let calls = 0
  session.agent.streamFunction = async (model, context, options) => {
    calls += 1
    const base = {
      role: 'assistant',
      api: model.api,
      provider: model.provider,
      model: model.id,
      timestamp: Date.now(),
      usage: {
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
    }
    if (calls > 1) {
      const message = { ...base, content: [{ type: 'text', text: '好的。' }], stopReason: 'stop' }
      return {
        async *[Symbol.asyncIterator]() {
          yield { type: 'done', reason: 'stop', message }
        },
        result: async () => message,
      }
    }
    const partial = {
      ...base,
      stopReason: 'stop',
      content: [
        { type: 'thinking', thinking: '想一想' },
        { type: 'text', text: '先说一句。' },
        { type: 'toolCall', id: 'call_1', name: 'bash', arguments: { command: 'npm te' } },
      ],
    }
    let final = null
    return {
      async *[Symbol.asyncIterator]() {
        yield { type: 'start', partial }
        await new Promise((resolve) => {
          if (options.signal.aborted) resolve()
          else options.signal.addEventListener('abort', resolve, { once: true })
        })
        final = { ...partial, stopReason: 'aborted', errorMessage: 'Request was aborted' }
        yield { type: 'error', reason: 'aborted', error: final }
      },
      result: async () => final,
    }
  }
  installStepInterrupt(session)
  const run = session.agent.prompt('开始')
  await new Promise((resolve) => setTimeout(resolve, 20))
  session.agent.steer({
    role: 'user',
    content: [{ type: 'text', text: '停' }],
    timestamp: Date.now(),
  })
  assert.equal(interruptCurrentStep(session), true)
  await run
  const interrupted = conversation(session.agent.state.messages)[1]
  assert.equal(interrupted.stopReason, 'stop')
  assert.equal(interrupted.errorMessage, undefined)
  assert.deepEqual(interrupted.content, [{ type: 'text', text: '先说一句。' }])
  assert.equal(calls, 2)
})

test('send now aborts the running tool through its own signal and the run reads the message next', async (t) => {
  const { session } = await realSession(t)
  let toolSignal = null
  const tool = {
    name: 'slow',
    label: 'slow',
    description: 'slow tool',
    parameters: { type: 'object', properties: {} },
    execute: (id, args, signal) =>
      new Promise((resolve) => {
        toolSignal = signal
        signal.addEventListener('abort', () =>
          resolve({ content: [{ type: 'text', text: 'stopped' }], details: {} }),
        )
      }),
  }
  session.agent.state.tools = [tool]
  let calls = 0
  session.agent.streamFunction = async (model) => {
    calls += 1
    const base = {
      role: 'assistant',
      api: model.api,
      provider: model.provider,
      model: model.id,
      timestamp: Date.now(),
      usage: {
        input: 0,
        output: 0,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 0,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
    }
    const message =
      calls === 1
        ? {
            ...base,
            stopReason: 'toolUse',
            content: [{ type: 'toolCall', id: 'call_slow', name: 'slow', arguments: {} }],
          }
        : { ...base, stopReason: 'stop', content: [{ type: 'text', text: '收到。' }] }
    return {
      async *[Symbol.asyncIterator]() {
        yield { type: 'done', reason: message.stopReason, message }
      },
      result: async () => message,
    }
  }
  installStepInterrupt(session)
  const run = session.agent.prompt('跑个慢命令')
  for (let i = 0; i < 50 && !toolSignal; i += 1)
    await new Promise((resolve) => setTimeout(resolve, 10))
  assert.ok(toolSignal, 'the tool started')
  session.agent.steer({
    role: 'user',
    content: [{ type: 'text', text: '别跑了' }],
    timestamp: Date.now(),
  })
  assert.equal(interruptCurrentStep(session), true)
  await run
  assert.equal(toolSignal.aborted, true)
  assert.deepEqual(
    conversation(session.agent.state.messages).map((message) => message.role),
    ['user', 'assistant', 'toolResult', 'user', 'assistant'],
  )
})

test('send now on a queued item moves it to the front of the steering queue, from either queue', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-send-now-'))
  const { session } = await realSession(t)
  session._isAgentRunActive = true
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set(session.sessionId, { session, cwd: directory })
  runtime.selectToolsForMessage = () => {}
  runtime.archiveAttachments = async () => []
  runtime.liveSessions.set(session.sessionId, { streaming: true, queuedInputs: [] })
  t.after(async () => {
    session._isAgentRunActive = false
    runtime.sessions.clear()
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const enqueue = (message, behavior) =>
    runtime.queueSessionMessage(session.sessionId, { message, behavior })
  const first = await enqueue('第一条', 'steer')
  const second = await enqueue('第二条', 'steer')
  const later = await enqueue('整轮之后', 'followUp')
  const text = (message) => message.content.find((part) => part.type === 'text').text
  const sent = runtime.sendQueuedSessionMessageNow(session.sessionId, second.inputId)
  assert.equal(sent.sent, true)
  assert.deepEqual(session.agent.steeringQueue.messages.map(text), ['第二条', '第一条'])
  assert.deepEqual(
    sent.queuedInputs.map((item) => item.id),
    [second.inputId, first.inputId, later.inputId],
  )
  assert.equal(promoteQueuedSessionInput(session, later.inputId), true)
  assert.deepEqual(session.agent.steeringQueue.messages.map(text), ['整轮之后', '第二条', '第一条'])
  assert.equal(session.agent.followUpQueue.messages.length, 0)
  assert.deepEqual(
    [...session.getSteeringMessages()].sort(),
    ['整轮之后', '第一条', '第二条'].sort(),
  )
  assert.equal(session.getFollowUpMessages().length, 0)
  assert.equal(runtime.sendQueuedSessionMessageNow(session.sessionId, 'missing').sent, false)
  // 插队（behavior: interrupt）也排到队首：有消息先排着时，送出去的仍是插队的这条。
  const cut = await enqueue('插队这条', 'interrupt')
  assert.equal(session.agent.steeringQueue.messages.map(text)[0], '插队这条')
  assert.equal(cut.queuedInputs[0].id, cut.inputId)
})
