import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

function request(method, body) {
  return {
    method,
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    writeHead(status) {
      this.status = status
    },
    end(body = '') {
      this.body = body
    },
  }
}

test('running sessions accept steering and follow-up user messages through the Pi queue', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-session-input-'))
  let runtime
  t.after(async () => {
    await runtime?.dispose?.().catch(() => {})
    await rm(directory, { recursive: true, force: true }).catch(() => {})
  })
  const calls = []
  const steering = []
  const followUp = []
  const session = {
    isStreaming: true,
    pendingMessageCount: 2,
    getSteeringMessages: () => steering,
    getFollowUpMessages: () => followUp,
    async prompt(message, options) {
      calls.push({ message, options })
      if (options.streamingBehavior === 'followUp') followUp.push(message)
      else steering.push(message)
    },
  }
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  const selections = []
  runtime.selectToolsForMessage = (_value, message, options) => {
    selections.push({ message, options })
  }
  runtime.sessions.set('session-1', { session, modified: '' })

  assert.deepEqual(
    await runtime.queueSessionMessage('session-1', {
      message: 'Focus on the Windows path.',
      behavior: 'steer',
    }),
    {
      queued: true,
      behavior: 'steer',
      pendingMessageCount: 2,
      queuedInputs: [{ behavior: 'steer', text: 'Focus on the Windows path.' }],
    },
  )
  assert.equal(calls[0].message, 'Focus on the Windows path.')
  assert.deepEqual(calls[0].options, { streamingBehavior: 'steer', source: 'interactive' })
  assert.deepEqual(selections[0], {
    message: 'Focus on the Windows path.',
    options: { preserveRequested: true },
  })

  await runtime.queueSessionMessage('session-1', {
    message: 'Then update the tests.',
    behavior: 'followUp',
  })
  assert.equal(calls[1].options.streamingBehavior, 'followUp')

  session.isStreaming = false
  await assert.rejects(
    runtime.queueSessionMessage('session-1', { message: 'Too late.' }),
    /已经结束运行/,
  )
})

test('session input API delegates queued messages without opening another SSE response', async () => {
  const calls = []
  const runtime = {
    async queueSessionMessage(id, input) {
      calls.push({ id, input })
      return { queued: true, behavior: input.behavior, pendingMessageCount: 1, queuedInputs: [] }
    },
  }
  const handler = createApiHandler(runtime)
  const res = response()
  assert.equal(
    await handler(
      request('POST', { message: 'Keep going, but skip packaging.', behavior: 'steer' }),
      res,
      new URL('http://localhost/api/sessions/session%201/input'),
    ),
    true,
  )
  assert.equal(res.status, 200)
  assert.deepEqual(JSON.parse(res.body), {
    queued: true,
    behavior: 'steer',
    pendingMessageCount: 1,
    queuedInputs: [],
  })
  assert.deepEqual(calls, [
    {
      id: 'session 1',
      input: { message: 'Keep going, but skip packaging.', behavior: 'steer' },
    },
  ])
})
