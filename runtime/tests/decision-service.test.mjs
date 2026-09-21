// 决策服务测试：远端协议（请求形状、错误映射、超限拦截）、配置持久化
// 与 HTTP 路由契约。不访问真实网络。
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'

import {
  DecisionError,
  assertWithinRemoteLimit,
  callRemoteDecisions,
  estimateTokens,
  normalizeDecideInput,
  remoteEndpoint,
  toRemoteQuestions,
} from '../services/decision-remote-client.mjs'
import { DecisionService } from '../services/decision-service.mjs'
import { createApiHandler } from '../http/api-handler.mjs'

function jsonResponse(status, payload) {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => JSON.stringify(payload),
  }
}

const REMOTE_CONFIG = {
  provider: 'typesafe',
  baseUrl: '',
  modelId: 'jev-1.13.0',
  apiKey: 'sk-test',
}

test('normalizeDecideInput 校验问题类型与边界', () => {
  const ok = normalizeDecideInput({
    state: { text: 'hello' },
    questions: [
      { type: 'noul', instructions: 'Is it positive?' },
      { id: 'area', type: 'choice', instructions: 'Pick area', options: ['a', 'b'] },
      { type: 'score', instructions: 'Rate', options: ['bad', 'ok', 'good'] },
    ],
  })
  assert.equal(ok.state, '{"text":"hello"}')
  assert.deepEqual(Object.keys(ok.questions), ['q0', 'area', 'q2'])

  assert.throws(
    () => normalizeDecideInput({ state: '', questions: [{ type: 'noul', instructions: 'x' }] }),
    DecisionError,
  )
  assert.throws(() => normalizeDecideInput({ state: 's', questions: [] }), /非空数组/)
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [{ type: 'choice', instructions: 'x', options: ['a'] }],
      }),
    /2–255/,
  )
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [{ type: 'score', instructions: 'x', options: ['a'] }],
      }),
    /2–10/,
  )
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [{ type: 'choice', instructions: 'x', options: ['a', 'a'] }],
      }),
    /重复选项/,
  )
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [{ id: '1bad', type: 'noul', instructions: 'x' }],
      }),
    /问题 id/,
  )
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [
          { id: 'dup', type: 'noul', instructions: 'x' },
          { id: 'dup', type: 'noul', instructions: 'y' },
        ],
      }),
    /重复/,
  )
  assert.throws(
    () => normalizeDecideInput({ state: 's', questions: [{ type: 'bogus', instructions: 'x' }] }),
    /noul \/ choice \/ score/,
  )
})

test('入口限长：超过字符上限直接驳回，不截断', () => {
  // state 超过 200k 字符
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 'x'.repeat(200_001),
        questions: [{ type: 'noul', instructions: 'x' }],
      }),
    (error) => error.code === 'state_too_large',
  )
  // instructions 超过 2000 字符
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [{ type: 'noul', instructions: 'x'.repeat(2001) }],
      }),
    /instructions 过长/,
  )
  // 选项标签超过 200 字符
  assert.throws(
    () =>
      normalizeDecideInput({
        state: 's',
        questions: [{ type: 'choice', instructions: 'x', options: ['y'.repeat(201), 'z'] }],
      }),
    /选项标签过长/,
  )
  // 超限输入不会被修改：合法输入原样保留
  const ok = normalizeDecideInput({
    state: 'a'.repeat(199_999),
    questions: [{ type: 'noul', instructions: 'full content preserved' }],
  })
  assert.equal(ok.state.length, 199_999)
})

test('judgeToolCall 不截断参数：超限直接驳回', async (t) => {
  const service = await makeService(t, {
    fetchImpl: async () => jsonResponse(200, { answers: { approve: { noul: 1 } } }),
  })
  await service.updateConfig({ remote: { apiKey: 'sk-x' }, delegate: { enabled: true } })
  await assert.rejects(
    service.judgeToolCall({ toolName: 'write', args: { content: 'x'.repeat(500_000) } }),
    (error) => error.code === 'state_too_large',
  )
})

test('toRemoteQuestions 映射为官方协议形状', () => {
  const { questions } = normalizeDecideInput({
    state: 's',
    questions: [
      { id: 'sentiment', type: 'choice', instructions: 'Sentiment?', options: ['pos', 'neg'] },
      { id: 'stars', type: 'score', instructions: 'Stars?', options: ['1', '2', '3'] },
      { id: 'refund', type: 'noul', instructions: 'Refund asked?' },
    ],
  })
  const remote = toRemoteQuestions(questions)
  assert.deepEqual(remote.sentiment.criteria, { pos: null, neg: null })
  assert.deepEqual(remote.stars.criteria, ['1', '2', '3'])
  assert.equal(remote.refund.type, 'noul')
  assert.equal(remote.refund.criteria, undefined)
})

test('remoteEndpoint 按 provider 拼接路径，完整地址原样使用', () => {
  assert.equal(remoteEndpoint(REMOTE_CONFIG), 'https://api.typesafe.ai/v1/systemone')
  assert.equal(
    remoteEndpoint({ provider: 'openrouter', modelId: 'm', apiKey: 'k' }),
    'https://openrouter.ai/api/alpha/decisions',
  )
  assert.equal(
    remoteEndpoint({
      provider: 'custom',
      baseUrl: 'https://relay.example.com/v1/systemone',
      modelId: 'm',
      apiKey: 'k',
    }),
    'https://relay.example.com/v1/systemone',
  )
  assert.throws(() =>
    remoteEndpoint({
      provider: 'custom',
      baseUrl: 'http://insecure.example.com',
      modelId: 'm',
      apiKey: 'k',
    }),
  )
})

test('estimateTokens 区分中英文密度', () => {
  assert.ok(estimateTokens('你好世界') > estimateTokens('abcd'))
})

test('assertWithinRemoteLimit 拦截超限输入', () => {
  assert.throws(
    () => assertWithinRemoteLimit('好'.repeat(30000), { q: { type: 'noul', instructions: 'x' } }),
    (error) => error.code === 'state_too_large',
  )
  assertWithinRemoteLimit('short state', { q: { type: 'noul', instructions: 'x' } })
})

test('callRemoteDecisions 归一化三种原语的响应', async () => {
  let requestBody = null
  const fetchImpl = async (_url, init) => {
    requestBody = JSON.parse(init.body)
    return jsonResponse(200, {
      model: 'jev-1.13.0',
      answers: {
        refund: { noul: 0.9 },
        area: { choice: 'fees', probabilities: { fees: 0.8, other: 0.2 }, confidence: 0.8 },
        stars: {
          score: 1.4,
          legend: { 0: 'bad', 1: 'ok', 2: 'good' },
          probabilities: {},
          confidence: 0.5,
        },
      },
      usage: { input_tokens: 123, cost: 0.00001 },
    })
  }
  const result = await callRemoteDecisions(
    REMOTE_CONFIG,
    normalizeDecideInput({
      state: 'I want my money back',
      questions: [
        { id: 'refund', type: 'noul', instructions: 'Refund?' },
        { id: 'area', type: 'choice', instructions: 'Area?', options: ['fees', 'other'] },
        { id: 'stars', type: 'score', instructions: 'Rate', options: ['bad', 'ok', 'good'] },
      ],
    }),
    { fetchImpl },
  )
  assert.equal(requestBody.model, 'jev-1.13.0')
  assert.equal(result.answers.refund.noul, 0.9)
  assert.equal(result.answers.area.choice, 'fees')
  assert.equal(result.answers.stars.score, 1.4)
  assert.equal(result.usage.inputTokens, 123)
  assert.equal(result.usage.costUsd, 0.00001)
  assert.equal(result.model, 'jev-1.13.0')
})

test('callRemoteDecisions 错误映射：401 不重试，429 退避后成功', async () => {
  let calls = 0
  const authFetch = async () => {
    calls += 1
    return jsonResponse(401, { error: { message: 'bad key' } })
  }
  await assert.rejects(
    callRemoteDecisions(
      REMOTE_CONFIG,
      normalizeDecideInput({ state: 's', questions: [{ type: 'noul', instructions: 'x' }] }),
      { fetchImpl: authFetch },
    ),
    (error) => error.code === 'auth',
  )
  assert.equal(calls, 1)

  calls = 0
  const flakyFetch = async () => {
    calls += 1
    if (calls < 3) return jsonResponse(429, { error: { message: 'slow down' } })
    return jsonResponse(200, { answers: { q0: { noul: 0.5 } } })
  }
  const result = await callRemoteDecisions(
    REMOTE_CONFIG,
    normalizeDecideInput({ state: 's', questions: [{ type: 'noul', instructions: 'x' }] }),
    { fetchImpl: flakyFetch },
  )
  assert.equal(calls, 3)
  assert.equal(result.answers.q0.noul, 0.5)
})

test('callRemoteDecisions 缺少密钥直接报配置错误', async () => {
  await assert.rejects(
    callRemoteDecisions(
      { provider: 'typesafe', modelId: 'jev-1.13.0', apiKey: '' },
      normalizeDecideInput({ state: 's', questions: [{ type: 'noul', instructions: 'x' }] }),
      { fetchImpl: async () => jsonResponse(200, {}) },
    ),
    (error) => error.code === 'config_missing',
  )
})

async function makeService(t, extra = {}) {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-decisions-'))
  t.after(async () => {
    await rm(dataDir, { recursive: true, force: true })
  })
  const service = new DecisionService({ dataDir, ...extra })
  await service.init()
  return service
}

test('DecisionService 配置：密钥不回传，空密钥保留，null 清除', async (t) => {
  const service = await makeService(t)
  let config = service.publicConfig()
  assert.equal(config.remote.hasKey, false)
  assert.equal(config.remote.baseUrl, 'https://api.typesafe.ai')
  assert.equal(config.remote.modelId, 'jev-1.13.0')

  config = await service.updateConfig({
    remote: { provider: 'openrouter', apiKey: 'sk-or-secret' },
  })
  assert.equal(config.remote.hasKey, true)
  assert.equal(config.remote.modelId, 'typesafe/jev-1.13')
  assert.equal(config.remote.baseUrl, 'https://openrouter.ai/api')
  assert.equal(JSON.stringify(config).includes('sk-or-secret'), false)

  // 空字符串表示保持现有密钥
  config = await service.updateConfig({ remote: { apiKey: '', modelId: 'typesafe/jev-2' } })
  assert.equal(config.remote.hasKey, true)
  assert.equal(config.remote.modelId, 'typesafe/jev-2')

  // null 表示清除
  config = await service.updateConfig({ remote: { apiKey: null } })
  assert.equal(config.remote.hasKey, false)

  // 重启后配置仍在（持久化）
  const reloaded = new DecisionService({ dataDir: service.dir.replace(/\/decisions$/, '') })
  await reloaded.init()
  assert.equal(reloaded.publicConfig().remote.modelId, 'typesafe/jev-2')

  await assert.rejects(service.updateConfig({ backend: 'sideways' }), /未知的配置字段/)
})

test('DecisionService decide 走注入的 fetch，密钥不进入 URL', async (t) => {
  let seen = null
  const fetchImpl = async (url, init) => {
    seen = { url, init }
    return jsonResponse(200, { answers: { q0: { noul: 0.7 } }, usage: { input_tokens: 10 } })
  }
  const service = await makeService(t, { fetchImpl })
  await service.updateConfig({ remote: { provider: 'openrouter', apiKey: 'sk-hidden' } })
  const result = await service.decide({
    state: 's',
    questions: [{ type: 'noul', instructions: 'x' }],
  })
  assert.equal(result.backend, 'remote')
  assert.equal(result.answers.q0.noul, 0.7)
  assert.equal(seen.url, 'https://openrouter.ai/api/alpha/decisions')
  assert.equal(seen.init.headers.Authorization, 'Bearer sk-hidden')
  assert.ok(!seen.url.includes('sk-hidden'))
})

test('DecisionService 委派配置：校验阈值范围', async (t) => {
  const service = await makeService(t)
  assert.deepEqual(service.publicConfig().delegate, {
    enabled: false,
    allowThreshold: 0.9,
    verifyActions: false,
  })
  assert.equal(service.delegationEnabled(), false)

  const config = await service.updateConfig({
    delegate: { enabled: true, allowThreshold: 0.8 },
  })
  assert.equal(config.delegate.enabled, true)
  assert.equal(config.delegate.allowThreshold, 0.8)
  assert.equal(service.delegationEnabled(), true)

  await assert.rejects(service.updateConfig({ delegate: { allowThreshold: 0.3 } }), /0.5–1/)
  // enabled 之外的字段更新后开关保持
  const kept = await service.updateConfig({ delegate: { allowThreshold: 0.95 } })
  assert.equal(kept.delegate.enabled, true)
})

test('DecisionService judgeToolCall：模型只能授予便利，其余一律 ask', async (t) => {
  const probabilities = [0.97, 0.03, 0.5]
  const fetchImpl = async () =>
    jsonResponse(200, { answers: { approve: { noul: probabilities.shift() } } })
  const service = await makeService(t, { fetchImpl })
  await service.updateConfig({
    remote: { apiKey: 'sk-x' },
    delegate: { enabled: true, allowThreshold: 0.9 },
  })
  assert.equal(
    (await service.judgeToolCall({ toolName: 'bash', args: { command: 'ls' } })).verdict,
    'approve',
  )
  // 低置信度不拒绝，返回 ask 由会话权限模式接管
  assert.equal(
    (await service.judgeToolCall({ toolName: 'bash', args: { command: 'rm -rf /' } })).verdict,
    'ask',
  )
  assert.equal((await service.judgeToolCall({ toolName: 'edit' })).verdict, 'ask')
})

test('SessionPermissionService 委派：approve 免审批，其余一律继承会话权限模式', async () => {
  const { SessionPermissionService } = await import('../services/session-permission-service.mjs')
  const ask = async (verdicts) => {
    let asked = 0
    const service = new SessionPermissionService({
      getMode: () => 'ask',
      getExecutionMode: () => 'workspace-write',
      decideDelegation: async () => verdicts.shift() ?? 'ask',
    })
    service.requestApproval = async () => {
      asked += 1
      return { approved: true }
    }
    const authorize = () =>
      service.authorize({
        sessionId: 's1',
        cwd: process.cwd(),
        toolName: 'bash',
        toolCallId: 'c1',
        args: { command: 'echo hi' },
      })
    return { authorize, asked: () => asked }
  }

  // approve：直接放行，不请求人工
  let session = await ask(['approve'])
  assert.equal(await session.authorize(), undefined)
  assert.equal(session.asked(), 0)

  // ask：继承会话权限模式，照常人工审批
  session = await ask(['ask'])
  assert.equal(await session.authorize(), undefined)
  assert.equal(session.asked(), 1)

  // 未知返回值（含历史上的 deny）：一律按 ask 继承处理，不产生模型否决
  session = await ask(['deny'])
  assert.equal(await session.authorize(), undefined)
  assert.equal(session.asked(), 1)

  // 委派抛异常：回落人工审批
  const failing = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => 'workspace-write',
    decideDelegation: async () => {
      throw new Error('network down')
    },
  })
  let asked = 0
  failing.requestApproval = async () => {
    asked += 1
    return { approved: true }
  }
  await failing.authorize({
    sessionId: 's1',
    cwd: process.cwd(),
    toolName: 'bash',
    toolCallId: 'c1',
    args: { command: 'echo hi' },
  })
  assert.equal(asked, 1)
})

test('DecisionService 动作验证：开关+密钥门控与判断映射', async (t) => {
  const service = await makeService(t, {
    fetchImpl: async () => jsonResponse(200, { answers: { met: { noul: 0.93 } } }),
  })
  // 默认关闭
  assert.equal(service.actionVerificationEnabled(), false)
  await service.updateConfig({ delegate: { verifyActions: true } })
  // 开关打开但没有密钥：不可用
  assert.equal(service.actionVerificationEnabled(), false)
  await service.updateConfig({ remote: { apiKey: 'sk-x' } })
  assert.equal(service.actionVerificationEnabled(), true)

  const verdict = await service.verifyActionOutcome({
    expectation: 'the dialog is dismissed',
    outcomeText: 'outline: window main, button Save',
  })
  assert.equal(verdict.passed, true)
  assert.equal(verdict.probability, 0.93)
})

test('computer-use-verify 纯函数：文本提取与验证结论追加', async () => {
  const { extractOutcomeText, appendVerification } =
    await import('../runtime/computer-use-verification.mjs')
  const result = {
    content: [
      { type: 'text', text: 'outline text' },
      { type: 'image', data: 'base64…' },
      { type: 'text', text: 'successor state' },
    ],
    details: { someField: 1 },
  }
  assert.equal(extractOutcomeText(result), 'outline text\nsuccessor state')
  assert.equal(extractOutcomeText({}), '')

  const passed = appendVerification(result, {
    status: 'passed',
    expectation: 'e',
    probability: 0.9,
  })
  assert.match(passed.content.at(-1).text, /PASSED \(p=0\.90\)/)
  assert.equal(passed.details.actionVerification.status, 'passed')
  assert.equal(passed.details.someField, 1)
  // 原结果不被修改
  assert.equal(result.content.length, 3)

  const failed = appendVerification(result, {
    status: 'failed',
    expectation: 'e',
    probability: 0.2,
  })
  assert.match(failed.content.at(-1).text, /FAILED.*Re-observe/)

  const unavailable = appendVerification(result, {
    status: 'unavailable',
    expectation: 'e',
    reason: 'config_missing',
  })
  assert.match(unavailable.content.at(-1).text, /unavailable: config_missing/)
})

test('typed_decide 工具：注册进目录并调用决策服务', async (t) => {
  const { createAppTools } = await import('../tools/registry.mjs')
  const service = await makeService(t, {
    fetchImpl: async () => jsonResponse(200, { answers: { q0: { noul: 0.8 } } }),
  })
  await service.updateConfig({ remote: { apiKey: 'sk-tool' } })

  const tools = createAppTools({ enabledTools: ['typed_decide'], decisionService: service })
  assert.equal(tools.length, 1)
  assert.equal(tools[0].name, 'typed_decide')

  const result = await tools[0].execute('call-1', {
    state: 'The deployment failed twice.',
    questions: [{ type: 'noul', instructions: 'Is this about a deployment issue?' }],
  })
  assert.equal(result.details.backend, 'remote')
  assert.equal(result.details.answers.q0.noul, 0.8)

  // 服务缺失时给 Agent 明确错误
  const [bare] = createAppTools({ enabledTools: ['typed_decide'] })
  await assert.rejects(
    bare.execute('call-2', { state: 's', questions: [{ type: 'noul', instructions: 'x' }] }),
    /not initialized/,
  )

  // 错误带稳定码，便于 Agent 分支处理
  const noKey = await makeService(t)
  const [noKeyTool] = createAppTools({ enabledTools: ['typed_decide'], decisionService: noKey })
  await assert.rejects(
    noKeyTool.execute('call-3', {
      state: 's',
      questions: [{ type: 'noul', instructions: 'x' }],
    }),
    /config_missing/,
  )
})

// ---- HTTP 路由契约 ----

function mockRes() {
  return {
    status: 0,
    body: null,
    headersSent: false,
    writableEnded: false,
    destroyed: false,
    writeHead(status) {
      this.status = status
      this.headersSent = true
    },
    end(payload) {
      this.body = payload
      this.writableEnded = true
    },
    setHeader() {},
    flushHeaders() {},
  }
}

function mockReq(payload) {
  const body = JSON.stringify(payload ?? {})
  return {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    async *[Symbol.asyncIterator]() {
      yield Buffer.from(body)
    },
  }
}

test('decisions 路由：status 不回传密钥，decide 拒绝未知字段', async (t) => {
  const service = await makeService(t, {
    fetchImpl: async () => jsonResponse(200, { answers: { q0: { noul: 0.5 } } }),
  })
  await service.updateConfig({ remote: { apiKey: 'sk-route-secret' } })
  const handler = createApiHandler({}, { decisions: service })

  // GET /api/decisions/status
  const statusRes = mockRes()
  await handler({ method: 'GET' }, statusRes, new URL('http://localhost/api/decisions/status'))
  assert.equal(statusRes.status, 200)
  const statusBody = JSON.parse(statusRes.body)
  assert.equal(JSON.stringify(statusBody).includes('sk-route-secret'), false)
  assert.equal(statusBody.config.remote.hasKey, true)

  // POST /api/decisions/decide 正常路径
  const decideRes = mockRes()
  await handler(
    mockReq({ state: 's', questions: [{ type: 'noul', instructions: 'x' }] }),
    decideRes,
    new URL('http://localhost/api/decisions/decide'),
  )
  assert.equal(decideRes.status, 200)
  assert.equal(JSON.parse(decideRes.body).answers.q0.noul, 0.5)

  // 未知字段被拒绝
  const badRes = mockRes()
  await handler(
    mockReq({ state: 's', questions: [{ type: 'noul', instructions: 'x' }], evil: true }),
    badRes,
    new URL('http://localhost/api/decisions/decide'),
  )
  assert.equal(badRes.status, 400)

  // 错误响应带稳定错误码
  const noKey = createApiHandler({}, { decisions: await makeService(t) })
  const missingRes = mockRes()
  await noKey(
    mockReq({ state: 's', questions: [{ type: 'noul', instructions: 'x' }] }),
    missingRes,
    new URL('http://localhost/api/decisions/decide'),
  )
  assert.equal(missingRes.status, 400)
  assert.equal(JSON.parse(missingRes.body).code, 'config_missing')

  // 无服务时 503
  const bare = createApiHandler({})
  const missingServiceRes = mockRes()
  await bare({ method: 'GET' }, missingServiceRes, new URL('http://localhost/api/decisions/status'))
  assert.equal(missingServiceRes.status, 503)
})

test('审批委派遇到命令或嵌套参数凭据时不访问远端并回落人工', async (t) => {
  let calls = 0
  const service = await makeService(t, {
    fetchImpl: async () => {
      calls += 1
      return jsonResponse(200, { answers: { approve: { noul: 1 } } })
    },
  })
  await service.updateConfig({ remote: { apiKey: 'test' }, delegate: { enabled: true } })
  for (const args of [
    { command: 'curl -H "Authorization: Bearer review-only-secret" https://example.invalid' },
    { command: 'curl -H "Authorization: Basic dXNlcjpwdw==" https://example.invalid' },
    { command: 'curl -H "Cookie: sid=test1234" https://example.invalid' },
    { command: 'curl -H "X-API-Key: synthetic" https://example.invalid' },
    { command: 'tool --password=synthetic-password' },
    { command: 'curl https://example.invalid/?access_token=synthetic-token' },
    { headers: { Authorization: 'Basic synthetic-credential' } },
    { connection: { clientSecret: 'synthetic-secret' } },
    { command: 'echo -----BEGIN PRIVATE KEY-----\nsynthetic\n-----END PRIVATE KEY-----' },
  ]) {
    assert.deepEqual(await service.judgeToolCall({ toolName: 'bash', args }), { verdict: 'ask' })
  }
  assert.equal(calls, 0)
  const { SessionPermissionService } = await import('../services/session-permission-service.mjs')
  const permissions = new SessionPermissionService({
    getMode: () => 'ask',
    getExecutionMode: () => 'workspace-write',
    decideDelegation: async (call) => (await service.judgeToolCall(call)).verdict,
  })
  let asked = 0
  permissions.requestApproval = async () => {
    asked += 1
    return { approved: false }
  }
  const result = await permissions.authorize({
    sessionId: 'test',
    cwd: '/tmp',
    toolName: 'bash',
    toolCallId: 'test',
    args: { command: 'curl -H "Authorization: Bearer review-only-secret" https://example.invalid' },
  })
  assert.equal(result.block, true)
  assert.equal(asked, 1)
  assert.equal(calls, 0)
  assert.equal(
    (
      await service.judgeToolCall({
        toolName: 'bash',
        args: { command: 'echo hello', maxTokens: 10 },
      })
    ).verdict,
    'approve',
  )
  assert.equal(calls, 1)
})
