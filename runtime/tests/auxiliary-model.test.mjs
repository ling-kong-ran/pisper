// 辅助模型：起标题、压缩上下文、记忆提取这类后台杂活改用设置里的模型，用不了时退回原来的模型。
import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'
import { normalizeAuxiliaryModelRef } from '../runtime/auxiliary-model.mjs'
import { pisperCompactionExtension } from '../runtime/compaction-policy.mjs'

test('auxiliary model refs accept provider/modelId only', () => {
  assert.equal(normalizeAuxiliaryModelRef(' a/x '), 'a/x')
  assert.equal(normalizeAuxiliaryModelRef('a/b/c'), 'a/b/c')
  for (const value of ['', 'bad', '/x', 'a/', 42, null, `a/${'x'.repeat(400)}`]) {
    assert.equal(normalizeAuxiliaryModelRef(value), '')
  }
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

function systemText(body) {
  const first = body.messages?.[0]
  return typeof first?.content === 'string' ? first.content : JSON.stringify(first?.content || '')
}

// 主模型与辅助模型两个 Provider 共用一个模拟服务，按路径区分并记下每次请求。
async function auxiliaryRuntime(t, { auxiliaryModel = '' } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-auxiliary-model-'))
  const requests = []
  const server = createServer(async (request, reply) => {
    if (request.method === 'GET' && request.url.endsWith('/models')) {
      reply.writeHead(200, { 'content-type': 'application/json' })
      reply.end(JSON.stringify({ data: [{ id: 'fixture' }] }))
      return
    }
    const parts = []
    for await (const part of request) parts.push(part)
    const body = JSON.parse(Buffer.concat(parts).toString('utf8'))
    const target = request.url.startsWith('/aux') ? 'aux' : 'main'
    const system = systemText(body)
    const kind = /short title/.test(system)
      ? 'title'
      : /memory candidates/.test(system)
        ? 'memory'
        : 'chat'
    requests.push({ target, kind })
    const text =
      kind === 'title' ? '“重构登录校验。”' : kind === 'memory' ? '[]' : '好的，马上处理。'
    reply.writeHead(200, { 'content-type': 'text/event-stream' })
    reply.write(`data: ${JSON.stringify(chunk({ role: 'assistant', content: text }))}\n\n`)
    reply.write(
      `data: ${JSON.stringify({ ...chunk({}, 'stop'), usage: { prompt_tokens: 12, completion_tokens: 4, total_tokens: 16 } })}\n\n`,
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
    JSON.stringify({ providers: { 'test-main': provider('main'), 'test-aux': provider('aux') } }),
  )
  await writeFile(
    join(directory, 'auth.json'),
    JSON.stringify({
      'test-main': { type: 'api_key', key: 'test-only-main-key' },
      'test-aux': { type: 'api_key', key: 'test-only-aux-key' },
    }),
  )
  await writeFile(join(directory, 'settings.json'), JSON.stringify({ retry: { enabled: false } }))
  await writeFile(join(directory, 'pisper.json'), JSON.stringify({ auxiliaryModel }))
  runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.memorySummarizer = { summarize: async (entries) => entries.map(() => '') }
  await runtime.init()
  runtime.memory.setSemanticSummarizer(null)
  const created = await runtime.createSession('', directory)
  await runtime.setSessionModel(created.id, 'test-main', 'fixture')
  return { runtime, requests, sessionId: created.id }
}

// 后台任务（起标题、记忆提取）不阻塞这一轮：等它们全部收尾再断言。
async function settled(runtime) {
  const deadline = Date.now() + 10_000
  while (runtime.auxiliaryTitles.controllers.size || runtime.memoryCapture.tasks.size) {
    assert.ok(Date.now() < deadline, '后台任务应在期限内结束')
    await Promise.all([
      ...[...runtime.memoryCapture.tasks.keys()].map((task) => task.catch(() => {})),
      new Promise((resolve) => setTimeout(resolve, 20)),
    ])
  }
}

test('auxiliary model settings are validated on save and cleared with an empty string', async (t) => {
  const { runtime } = await auxiliaryRuntime(t)
  assert.equal((await runtime.getConfig()).auxiliaryModel, '')
  await assert.rejects(runtime.setAuxiliaryModel({}), { code: 'invalid' })
  await assert.rejects(runtime.setAuxiliaryModel({ model: 'nope' }), /provider\/modelId/)
  await assert.rejects(runtime.setAuxiliaryModel({ model: 'test-gone/fixture' }), {
    code: 'not_found',
  })
  const saved = await runtime.setAuxiliaryModel({ model: ' test-aux/fixture ' })
  assert.equal(saved.auxiliaryModel, 'test-aux/fixture')
  assert.equal((await runtime.resolveAuxiliaryModel())?.provider, 'test-aux')
  assert.equal((await runtime.setAuxiliaryModel({ model: '' })).auxiliaryModel, '')
  assert.equal(await runtime.resolveAuxiliaryModel(), null)
})

test('with an auxiliary model the first turn gets a generated title and chores skip the main model', async (t) => {
  const { runtime, requests, sessionId } = await auxiliaryRuntime(t, {
    auxiliaryModel: 'test-aux/fixture',
  })
  const events = []
  await runtime.streamPrompt({
    sessionId,
    message: '把登录页的表单校验重构一下，邮箱和密码的规则抽成一个函数，改完审一遍。',
    send: (event, data) => events.push({ event, data }),
  })
  await settled(runtime)
  // 对话本身仍然走会话的主模型；起标题与记忆提取只发给辅助模型。
  assert.deepEqual(
    requests.filter((item) => item.kind === 'chat').map((item) => item.target),
    ['main'],
  )
  assert.ok(requests.some((item) => item.kind === 'title' && item.target === 'aux'))
  assert.equal(
    requests.some((item) => item.kind !== 'chat' && item.target === 'main'),
    false,
  )
  // 规则标题先顶上，辅助模型的标题清洗掉引号与句号后替换它。
  assert.equal(events.find((item) => item.event === 'session_title')?.data.source, 'first_message')
  assert.equal(runtime.sessionMeta[sessionId].name, '重构登录校验')
  assert.equal(runtime.sessionMeta[sessionId].manual, false)
  const live = await runtime.getExistingSession(sessionId)
  assert.equal(live.name, '重构登录校验')
})

test('without an auxiliary model titles stay rule-based and no extra requests are made', async (t) => {
  const { runtime, requests, sessionId } = await auxiliaryRuntime(t)
  await runtime.streamPrompt({ sessionId, message: '看一下登录页。', send: () => {} })
  await settled(runtime)
  assert.equal(
    requests.some((item) => item.kind === 'title'),
    false,
  )
  assert.equal(
    requests.some((item) => item.target === 'aux'),
    false,
  )
  assert.equal(runtime.sessionMeta[sessionId].name, '看一下登录页')
  // 消息页带上运行标记，一轮结束后为 false（客户端据此决定是否改读 /live）。
  assert.equal((await runtime.getSessionMessagePage(sessionId)).streaming, false)
})

function compactionHarness({ auxiliary, auth = () => ({ ok: true, apiKey: 'fixture' }), fail }) {
  const calls = []
  let handler
  pisperCompactionExtension(
    { on: (event, callback) => event === 'session_before_compact' && (handler = callback) },
    {
      resolveAuxiliaryModel: async () => auxiliary,
      compactSession: async (preparation, model, apiKey) => {
        calls.push({ model: `${model.provider}/${model.id}`, apiKey })
        if (fail?.(model)) throw new Error('context too long')
        return { summary: model.id, firstKeptEntryId: 'kept', tokensBefore: 10, details: {} }
      },
    },
  )
  const session = { provider: 'main', id: 'big', contextWindow: 200_000, maxTokens: 8_192 }
  const compact = () =>
    handler(
      {
        preparation: { settings: { reserveTokens: 16_384 }, messagesToSummarize: [] },
        signal: new AbortController().signal,
      },
      {
        model: session,
        modelRegistry: { getApiKeyAndHeaders: async (model) => auth(model) },
      },
    )
  return { calls, compact }
}

test('compaction summaries prefer the auxiliary model and fall back to the session model', async () => {
  const auxiliary = { provider: 'aux', id: 'small', contextWindow: 32_000, maxTokens: 4_096 }
  const preferred = compactionHarness({ auxiliary })
  assert.equal((await preferred.compact()).compaction.summary, 'small')
  assert.deepEqual(
    preferred.calls.map((call) => call.model),
    ['aux/small'],
  )

  // 辅助模型写摘要失败（例如上下文装不下）：同一次压缩改用会话模型重写。
  const failing = compactionHarness({ auxiliary, fail: (model) => model.provider === 'aux' })
  assert.equal((await failing.compact()).compaction.summary, 'big')
  assert.deepEqual(
    failing.calls.map((call) => call.model),
    ['aux/small', 'main/big'],
  )

  // 辅助模型取不到认证：直接用会话模型，不发请求给辅助模型。
  const unauthorized = compactionHarness({
    auxiliary,
    auth: (model) => (model.provider === 'aux' ? { ok: false } : { ok: true, apiKey: 'fixture' }),
  })
  assert.equal((await unauthorized.compact()).compaction.summary, 'big')
  assert.deepEqual(
    unauthorized.calls.map((call) => call.model),
    ['main/big'],
  )

  // 没设置辅助模型：行为与原来一致。
  const unset = compactionHarness({ auxiliary: null })
  assert.equal((await unset.compact()).compaction.summary, 'big')
})
