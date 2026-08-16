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
    sessionId: 'session-1',
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
  runtime.archiveAttachments = async () => [null, { path: join(directory, 'diagram.png') }]
  const selections = []
  runtime.selectToolsForMessage = (_value, message, options) => {
    selections.push({ message, options })
  }
  runtime.sessions.set('session-1', { session, modified: '' })

  assert.deepEqual(
    await runtime.queueSessionMessage('session-1', {
      message: 'Focus on the Windows path.',
      attachments: [
        { kind: 'text', name: 'notes.md', text: 'Use the Win32 path.' },
        { kind: 'image', name: 'diagram.png', mimeType: 'image/png', data: 'aW1hZ2U=' },
      ],
      behavior: 'steer',
    }),
    {
      queued: true,
      behavior: 'steer',
      pendingMessageCount: 2,
      queuedInputs: [{ behavior: 'steer', text: 'Focus on the Windows path.' }],
    },
  )
  assert.match(calls[0].message, /^Focus on the Windows path\./)
  assert.match(calls[0].message, /\[Text attachment: notes\.md\]\nUse the Win32 path\./)
  assert.match(calls[0].message, /\[Image attachment\] diagram\.png/)
  assert.deepEqual(calls[0].options, {
    images: [{ type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' }],
    streamingBehavior: 'steer',
    source: 'interactive',
  })
  assert.deepEqual(selections[0], {
    message: 'Focus on the Windows path.',
    options: { preserveRequested: true },
  })

  await runtime.queueSessionMessage('session-1', {
    message: 'Then update the tests.',
    behavior: 'followUp',
  })
  assert.deepEqual(calls[1].options, {
    images: [],
    streamingBehavior: 'followUp',
    source: 'interactive',
  })

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
      request('POST', {
        message: 'Keep going, but skip packaging.',
        attachments: [{ kind: 'path', name: 'notes.md', path: '/workspace/notes.md' }],
        behavior: 'steer',
      }),
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
      input: {
        message: 'Keep going, but skip packaging.',
        attachments: [{ kind: 'path', name: 'notes.md', path: '/workspace/notes.md' }],
        behavior: 'steer',
      },
    },
  ])
})
