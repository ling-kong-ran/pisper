// 备用模型：主模型重试用尽或额度耗尽时切到备用模型，从失败处接着跑。
import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import {
  isQuotaError,
  MAX_FALLBACK_MODELS,
  normalizeFallbackModels,
} from '../runtime/model-fallback.mjs'

test('fallback model settings are trimmed, deduplicated and bounded', () => {
  assert.deepEqual(
    normalizeFallbackModels([' a/x ', 'a/x', 'bad', '/x', 'a/', 42, 'b/y', 'c/z']),
    ['a/x', 'b/y'].slice(0, MAX_FALLBACK_MODELS),
  )
  assert.deepEqual(normalizeFallbackModels('a/x'), [])
  assert.equal(isQuotaError({ stopReason: 'error', errorMessage: '429 insufficient_quota' }), true)
  assert.equal(isQuotaError({ stopReason: 'error', errorMessage: '503 overloaded' }), false)
  assert.equal(isQuotaError({ stopReason: 'stop', errorMessage: 'insufficient_quota' }), false)
})

function chunk(delta, finish_reason = null) {
  return {
    id: 'test',
    object: 'chat.completion.chunk',
    created: 0,
    model: 'fixture',
    choices: [{ index: 0, delta, finish_reason }],
  }
}

// 两个 Provider 共用一个模拟服务，按路径区分：/primary 按 failure 返回错误，/backup 正常回答。
async function fallbackRuntime(t, { failure, fallbackModels = ['test-backup/fixture'] }) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-model-fallback-'))
  const requests = []
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const chunks = []
    for await (const part of request) chunks.push(part)
    const target = request.url.startsWith('/primary') ? 'primary' : 'backup'
    requests.push({ target, body: JSON.parse(Buffer.concat(chunks).toString('utf8')) })
    if (target === 'primary') {
      reply.writeHead(failure.status, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ error: { message: failure.message, type: failure.type } }))
      return
    }
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    reply.write(
      `data: ${JSON.stringify(chunk({ role: 'assistant', content: '备用模型的回答' }))}\n\n`,
    )
    reply.write(
      `data: ${JSON.stringify({ ...chunk({}, 'stop'), usage: { prompt_tokens: 20, completion_tokens: 5, total_tokens: 25 } })}\n\n`,
    )
    reply.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  let runtime
  t.after(async () => {
    await runtime?.dispose()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  })
  const base = `http://127.0.0.1:${server.address().port}`
  const provider = (path) => ({
    api: 'openai-completions',
    baseUrl: `${base}/${path}/v1`,
    models: [
      {
        id: 'fixture',
        name: 'Fixture',
        reasoning: false,
        input: ['text'],
        contextWindow: 200000,
        maxTokens: 8192,
      },
    ],
  })
  await writeFile(
    join(directory, 'models.json'),
    JSON.stringify({
      providers: { 'test-primary': provider('primary'), 'test-backup': provider('backup') },
    }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({
      'test-primary': { type: 'api_key', key: 'test-only-primary-key' },
      'test-backup': { type: 'api_key', key: 'test-only-backup-key' },
    }),
  )
  // 同一模型只重试一次、几乎不等待，让测试跑得快。
  await writeFile(
    join(directory, 'settings.json'),
    JSON.stringify({ retry: { enabled: true, maxRetries: 1, baseDelayMs: 1 } }),
  )
  await writeFile(join(directory, 'pisper.json'), JSON.stringify({ fallbackModels }))
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const created = await runtime.createSession('备用', directory)
  await runtime.setSessionModel(created.id, 'test-primary', 'fixture')
  return { runtime, requests, sessionId: created.id }
}

test('an exhausted quota switches to the backup model at once and keeps the session on it', async (t) => {
  const { runtime, requests, sessionId } = await fallbackRuntime(t, {
    failure: {
      status: 429,
      message: 'You exceeded your current quota.',
      type: 'insufficient_quota',
    },
  })
  const events = []
  await runtime.streamPrompt({
    sessionId,
    message: '帮我看看这个问题',
    send: (event, data) => events.push({ event, data }),
  })

  // 额度耗尽不在主模型上重试：一次主模型请求，随后一次备用模型请求。
  assert.deepEqual(
    requests.map((item) => item.target),
    ['primary', 'backup'],
  )
  // 备用模型拿到的是同一段上下文：用户消息还在，失败的那次回复不在。
  const backupMessages = JSON.stringify(requests[1].body.messages)
  assert.match(backupMessages, /帮我看看这个问题/)
  assert.doesNotMatch(backupMessages, /exceeded your current quota/)
  const fallback = events.find((item) => item.event === 'model_fallback')
  assert.deepEqual(
    [fallback.data.from, fallback.data.model, fallback.data.provider, fallback.data.modelId],
    ['test-primary/fixture', 'test-backup/fixture', 'test-backup', 'fixture'],
  )
  const done = events.find((item) => item.event === 'done')
  assert.equal(done.data.text, '备用模型的回答')
  assert.equal(
    events.some((item) => item.event === 'error'),
    false,
  )
  // 会话绑定改到备用模型，重启后也保持。
  const live = await runtime.getExistingSession(sessionId)
  assert.equal(`${live.session.model.provider}/${live.session.model.id}`, 'test-backup/fixture')
  assert.equal(runtime.sessionMeta[sessionId].model, 'test-backup/fixture')
  assert.match(await readFile(live.session.sessionFile, 'utf8'), /"provider":"test-backup"/)
})

test('a transient outage retries the same model first, then falls back', async (t) => {
  const { runtime, requests, sessionId } = await fallbackRuntime(t, {
    failure: { status: 503, message: 'Service temporarily unavailable', type: 'server_error' },
  })
  const events = []
  await runtime.streamPrompt({
    sessionId,
    message: '继续',
    send: (event, data) => events.push({ event, data }),
  })
  assert.deepEqual(
    requests
      .map((item) => item.target)
      .filter((target, index, list) => list.indexOf(target) === index),
    ['primary', 'backup'],
  )
  assert.ok(requests.filter((item) => item.target === 'primary').length >= 2)
  assert.equal(events.find((item) => item.event === 'done').data.text, '备用模型的回答')
})

test('without a usable backup the original error is reported unchanged', async (t) => {
  const { runtime, requests, sessionId } = await fallbackRuntime(t, {
    failure: {
      status: 429,
      message: 'You exceeded your current quota.',
      type: 'insufficient_quota',
    },
    // 设置里的备用模型已经被删掉：运行时跳过，不切换也不报新的错。
    fallbackModels: ['test-gone/fixture'],
  })
  const events = []
  await runtime
    .streamPrompt({
      sessionId,
      message: '在吗',
      send: (event, data) => events.push({ event, data }),
    })
    .catch(() => {})
  assert.deepEqual(
    requests.map((item) => item.target),
    ['primary'],
  )
  assert.match(events.find((item) => item.event === 'error').data.message, /quota/)
  assert.equal(
    events.some((item) => item.event === 'model_fallback'),
    false,
  )
})

test('fallback settings are validated on save and exposed through the config view', async (t) => {
  const { runtime } = await fallbackRuntime(t, {
    failure: { status: 503, message: 'x', type: 'server_error' },
    fallbackModels: [],
  })
  const saved = await runtime.setFallbackModels({ models: ['test-backup/fixture'] })
  assert.deepEqual(saved.fallbackModels, ['test-backup/fixture'])
  assert.deepEqual((await runtime.getConfig()).fallbackModels, ['test-backup/fixture'])
  await assert.rejects(runtime.setFallbackModels({ models: ['test-gone/fixture'] }), {
    code: 'not_found',
  })
  await assert.rejects(runtime.setFallbackModels({ models: ['a/x', 'a/x'] }), /unique/)
  await assert.rejects(runtime.setFallbackModels({ models: ['a/x', 'b/y', 'c/z'] }), {
    code: 'limit',
  })
  await assert.rejects(runtime.setFallbackModels({}), { code: 'invalid' })
  assert.deepEqual((await runtime.setFallbackModels({ models: [] })).fallbackModels, [])
})
