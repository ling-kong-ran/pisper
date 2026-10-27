import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  MAX_OUTPUT_LIMIT_CONTINUATIONS,
  installOutputLimitContinuation,
  isOutputLimitContinuationMessage,
} from '../runtime/output-limit-continuation.mjs'
import {
  createAgentSession,
  createDefaultResourceLoader,
  SessionManager,
  SettingsManager,
} from '../runtime/pi-coding-agent.mjs'
import { pisperPromptExtension } from '../prompts/pisper-system-prompt.mjs'
import {
  isInternalParentMessage,
  serializeTranscriptMessages,
} from '../runtime/stream-projection.mjs'

function assistantMessage(text, stopReason, output) {
  return {
    role: 'assistant',
    content: [{ type: 'text', text }],
    stopReason,
    usage: { output },
    timestamp: Date.now(),
  }
}

function lengthStop(text = 'partial reply', output = 100) {
  return assistantMessage(text, 'length', output)
}

function sessionDouble(checkCompaction = async () => false) {
  const queued = []
  const listeners = new Set()
  const session = {
    model: { maxTokens: 100 },
    messages: [],
    _queuedForTest: queued,
    agent: {
      followUp(message) {
        queued.push(message)
      },
      hasQueuedMessages() {
        return queued.length > 0
      },
    },
    _checkCompaction: checkCompaction,
    subscribe(listener) {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    emit(event) {
      for (const listener of listeners) listener(event)
    },
  }
  return { session, queued }
}

// 复现 Pi 0.87.1 的回合结束循环：打满输出上限时 _checkCompaction 返回 false，
// 没有排队消息就结束本次 prompt。
async function runUntilPiWouldStop(session, respond) {
  const continuations = []
  while (true) {
    const assistant = respond(continuations.length)
    session.messages.push(assistant)
    const shouldContinue = await session._checkCompaction(assistant, true, [])
    if (!shouldContinue || !session.agent.hasQueuedMessages()) break
    continuations.push(session._queuedForTest.shift())
  }
  return { assistants: session.messages, continuations }
}

test('Pi stops a genuine max-output length turn when nothing queues a continuation', async () => {
  const { session } = sessionDouble()
  const outcome = await runUntilPiWouldStop(session, () => lengthStop())
  assert.equal(outcome.assistants.length, 1)
  assert.equal(outcome.assistants[0].stopReason, 'length')
  assert.equal(outcome.continuations.length, 0)
})

test('a max output token stop continues the same run instead of ending it', async () => {
  const { session } = sessionDouble()
  installOutputLimitContinuation(session)
  const outcome = await runUntilPiWouldStop(session, (continuations) =>
    continuations === 0 ? lengthStop('half', 100) : assistantMessage('finished', 'stop', 12),
  )
  assert.deepEqual(
    outcome.assistants.map((message) => message.stopReason),
    ['length', 'stop'],
  )
  assert.equal(outcome.continuations.length, 1)
  const prompt = outcome.continuations[0].content[0].text
  assert.equal(isOutputLimitContinuationMessage(prompt), true)
  assert.match(prompt, /maximum output token limit \(1\/3\)/)
  assert.equal(isInternalParentMessage(prompt), true)
  assert.deepEqual(
    serializeTranscriptMessages([
      { role: 'user', content: prompt, timestamp: 1 },
      outcome.assistants[1],
    ]).map((message) => message.text),
    ['finished'],
  )
})

test('Pi recovery of a below-limit length stop is left alone', async () => {
  const { session, queued } = sessionDouble(async (message) => message.usage.output < 100)
  installOutputLimitContinuation(session)
  const continued = await session._checkCompaction(lengthStop('short', 16), true, [])
  assert.equal(continued, true)
  assert.equal(queued.length, 0)
})

test('turn-boundary and pre-prompt compaction checks do not inject a continuation', async () => {
  const { session, queued } = sessionDouble()
  installOutputLimitContinuation(session)
  assert.equal(await session._checkCompaction(lengthStop()), false)
  assert.equal(await session._checkCompaction(lengthStop(), false), false)
  assert.equal(queued.length, 0)
})

test('Pi 1.4 default compaction arguments continue only inside post-run handling', async () => {
  const { session, queued } = sessionDouble()
  let fail = false
  session._handlePostAgentRun = async function () {
    if (fail) throw new Error('post-run failed')
    return this._checkCompaction(lengthStop())
  }
  installOutputLimitContinuation(session)
  assert.equal(await session._handlePostAgentRun(), true)
  assert.equal(queued.length, 1)
  queued.pop()
  assert.equal(await session._checkCompaction(lengthStop()), false)
  fail = true
  await assert.rejects(session._handlePostAgentRun(), /post-run failed/)
  assert.equal(await session._checkCompaction(lengthStop()), false)
  assert.equal(queued.length, 0)
})

test('consecutive max-output stops stop after the continuation cap', async () => {
  const { session, queued } = sessionDouble()
  installOutputLimitContinuation(session)
  for (let attempt = 1; attempt <= MAX_OUTPUT_LIMIT_CONTINUATIONS; attempt += 1) {
    assert.equal(await session._checkCompaction(lengthStop(), true, []), true)
    assert.match(queued.at(-1).content[0].text, new RegExp(`\\(${attempt}/3\\)`))
    queued.pop()
  }
  assert.equal(await session._checkCompaction(lengthStop(), true, []), false)
  assert.equal(queued.length, 0)
})

test('a completed turn resets the continuation cap', async () => {
  const { session, queued } = sessionDouble()
  installOutputLimitContinuation(session)
  await session._checkCompaction(lengthStop(), true, [])
  queued.pop()
  assert.equal(await session._checkCompaction(assistantMessage('done', 'stop', 8), true, []), false)
  assert.equal(await session._checkCompaction(lengthStop(), true, []), true)
  assert.match(queued.at(-1).content[0].text, /\(1\/3\)/)
})

test('a new user message resets the continuation cap', async () => {
  const { session, queued } = sessionDouble()
  installOutputLimitContinuation(session)
  for (let attempt = 0; attempt < MAX_OUTPUT_LIMIT_CONTINUATIONS; attempt += 1) {
    await session._checkCompaction(lengthStop(), true, [])
    queued.pop()
  }
  session.emit({
    type: 'message_start',
    message: { role: 'user', content: 'Please keep going yourself.', timestamp: 1 },
  })
  assert.equal(await session._checkCompaction(lengthStop(), true, []), true)
  assert.match(queued.at(-1).content[0].text, /\(1\/3\)/)
})

test('an output-limit continuation does not reset its own cap', async () => {
  const { session, queued } = sessionDouble()
  installOutputLimitContinuation(session)
  await session._checkCompaction(lengthStop(), true, [])
  // Pi 先取出续跑消息再发起下一轮；这条 user message_start 不能把计数清零。
  const continuation = queued.pop()
  session.emit({ type: 'message_start', message: continuation })
  assert.equal(await session._checkCompaction(lengthStop(), true, []), true)
  assert.match(queued.at(-1).content[0].text, /\(2\/3\)/)
})

test('a queued user follow-up keeps priority over an automatic continuation', async () => {
  const { session, queued } = sessionDouble()
  installOutputLimitContinuation(session)
  queued.push({ role: 'user', content: 'User follow-up', timestamp: 1 })
  assert.equal(await session._checkCompaction(lengthStop(), true, []), false)
  assert.equal(queued.length, 1)
  assert.equal(queued[0].content, 'User follow-up')
})

test('agent sessions install output-limit continuation at creation', async () => {
  const source = await readFile(new URL('../runtime/pi-coding-agent.mjs', import.meta.url), 'utf8')
  assert.match(source, /installOutputLimitContinuation\(created\?\.session\)/)
})

async function realSessionFixture(t, stopReasons) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-output-limit-'))
  let session
  t.after(async () => {
    session?.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const settingsManager = SettingsManager.inMemory({
    compaction: { enabled: false },
    retry: { enabled: false },
  })
  const beforeStarts = []
  const resourceLoader = await createDefaultResourceLoader({
    cwd: directory,
    agentDir: directory,
    settingsManager,
    noExtensions: true,
    noSkills: true,
    noPromptTemplates: true,
    noThemes: true,
    noContextFiles: true,
    extensionFactories: [
      pisperPromptExtension,
      (pi) => {
        pi.on('before_agent_start', (event, context) => {
          beforeStarts.push({
            messageType: event.messageType,
            hasPrompt: Object.hasOwn(event, 'prompt'),
            idle: context.isIdle(),
          })
        })
      },
    ],
  })
  await resourceLoader.reload()
  const model = {
    id: 'output-limit-fixture',
    provider: 'fixture',
    api: 'openai-completions',
    name: 'Output limit fixture',
    input: ['text'],
    reasoning: false,
    contextWindow: 128000,
    maxTokens: 100,
  }
  ;({ session } = await createAgentSession({
    cwd: directory,
    agentDir: directory,
    model,
    modelRuntime: { hasConfiguredAuth: () => true, getModel: () => model },
    tools: [],
    sessionManager: SessionManager.inMemory(directory),
    settingsManager,
    resourceLoader,
  }))
  const requests = []
  // 只替换网络边界，保留 SDK 的事件、持久化投影和回合续跑循环。
  session.agent.streamFunction = async (activeModel, context) => {
    const stopReason = stopReasons[requests.length]
    assert.ok(stopReason, 'The session must not make an unexpected extra request')
    requests.push(structuredClone(context))
    const output = stopReason === 'length' ? activeModel.maxTokens : 1
    const message = {
      ...assistantMessage(stopReason === 'length' ? 'partial' : 'finished', stopReason, output),
      api: activeModel.api,
      provider: activeModel.provider,
      model: activeModel.id,
      usage: {
        input: 1,
        output,
        cacheRead: 0,
        cacheWrite: 0,
        totalTokens: 1 + output,
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
      },
    }
    return {
      async *[Symbol.asyncIterator]() {
        yield { type: 'done', reason: stopReason, message }
      },
      result: async () => message,
    }
  }
  return { session, requests, beforeStarts }
}

test('a real prompt finishes after a max-output continuation in the same call', async (t) => {
  const { session, requests, beforeStarts } = await realSessionFixture(t, ['length', 'stop'])

  await session.prompt('Finish the original task.')

  assert.equal(requests.length, 2)
  assert.deepEqual(beforeStarts, [{ messageType: 'user', hasPrompt: true, idle: false }])
  const continuation = requests[1].messages.find(
    (message) =>
      message.role === 'user' &&
      Array.isArray(message.content) &&
      message.content.some(
        (part) => part.type === 'text' && isOutputLimitContinuationMessage(part.text),
      ),
  )
  assert.ok(continuation)
  assert.equal(isInternalParentMessage(continuation.content[0].text), true)
  assert.deepEqual(
    session.messages
      .filter((message) => message.role === 'assistant')
      .map((message) => message.stopReason),
    ['length', 'stop'],
  )
  assert.equal(session.agent.hasQueuedMessages(), false)
  assert.equal(session.isStreaming, false)
})

test('custom-triggered runs retain the Pisper identity without a user prompt event', async (t) => {
  const { session, requests, beforeStarts } = await realSessionFixture(t, ['stop', 'stop'])

  await session.prompt('Start a user turn.')
  await session.sendCustomMessage(
    { customType: 'fixture', content: 'Continue from the extension.', display: false },
    { triggerTurn: true },
  )

  assert.equal(requests.length, 2)
  assert.deepEqual(beforeStarts, [
    { messageType: 'user', hasPrompt: true, idle: false },
    { messageType: 'custom', hasPrompt: false, idle: false },
  ])
  for (const request of requests) {
    const systemPrompt = request.messages
      .filter((message) => message.role === 'system')
      .map((message) => message.content)
      .join('\n')
    assert.match(systemPrompt, /Application: Pisper/)
    assert.match(systemPrompt, /Active provider: fixture/)
    assert.match(systemPrompt, /Active model: output-limit-fixture/)
  }
  assert.equal(session.isStreaming, false)
})
