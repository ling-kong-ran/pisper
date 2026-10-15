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
import { createAgentSession, SessionManager, SettingsManager } from '../runtime/pi-coding-agent.mjs'
import {
  isInternalParentMessage,
  serializeTranscriptMessages,
} from '../runtime/stream-projection.mjs'

const { createExtensionRuntime } = await import(
  new URL('./core/extensions/loader.js', import.meta.resolve('@earendil-works/pi-coding-agent'))
    .href
)

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

test('a real session continues after max output tokens when Pi will not retry', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-output-limit-'))
  const extensions = { extensions: [], errors: [], runtime: createExtensionRuntime() }
  const { session } = await createAgentSession({
    cwd: directory,
    agentDir: directory,
    model: {
      id: 'output-limit-fixture',
      provider: 'fixture',
      api: 'openai-completions',
      name: 'Output limit fixture',
      input: ['text'],
      reasoning: false,
      contextWindow: 128000,
      maxTokens: 100,
    },
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
      getSystemPrompt: () => 'Output limit fixture',
      getAppendSystemPrompt: () => [],
      extendResources() {},
      async reload() {},
    },
  })
  t.after(async () => {
    session.dispose()
    await rm(directory, { recursive: true, force: true })
  })

  const stopped = await session._checkCompaction(assistantMessage('done', 'stop', 8), true, [])
  assert.equal(stopped, false)
  assert.equal(session.agent.followUpQueue.messages.length, 0)

  const continued = await session._checkCompaction(lengthStop('half', 100), true, [])
  assert.equal(continued, true)
  const queued = session.agent.followUpQueue.messages
  assert.equal(queued.length, 1)
  assert.equal(queued[0].role, 'user')
  assert.equal(isOutputLimitContinuationMessage(queued[0].content[0].text), true)
  assert.equal(isInternalParentMessage(queued[0].content[0].text), true)
})
