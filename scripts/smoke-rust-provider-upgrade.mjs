// 用真实进程验证 Node 配置升级、导出导入、重启和凭据脱敏；只使用临时目录与虚构 Key。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdtemp, mkdir, writeFile, readFile, readdir, realpath, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { resolve, join } from 'node:path'
import { createHash } from 'node:crypto'
import { createServer } from 'node:http'
import { setTimeout as delay } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(process.argv[2] || join(root, 'runtime-rs/target/debug/pisper-server'))
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-provider-upgrade-'))
const agent = join(sandbox, 'agent')
const workspace = join(sandbox, 'workspace')
const primary = 'synthetic-upgrade-primary'
const secondary = 'synthetic-upgrade-secondary'
const id = (key) => createHash('sha256').update(key).digest('hex').slice(0, 24)
const token = 'synthetic_upgrade_isolation_token'
let child, ended, base
const modelRequests = []
const upstream = createServer(async (request, response) => {
  const chunks = []
  for await (const chunk of request) chunks.push(chunk)
  if (new URL(request.url, 'http://127.0.0.1').pathname !== '/v1/messages') {
    response.writeHead(404)
    response.end()
    return
  }
  const body = JSON.parse(Buffer.concat(chunks).toString())
  modelRequests.push(body)
  if (
    body.thinking?.type !== 'adaptive' ||
    body.thinking.budget_tokens !== undefined ||
    !body.output_config?.effort ||
    body.temperature !== undefined
  ) {
    response.writeHead(400, { 'Content-Type': 'application/json' })
    response.end(
      JSON.stringify({
        error: {
          type: 'invalid_request_error',
          message: 'Opus fixture requires adaptive thinking and effort without temperature',
        },
      }),
    )
    return
  }
  response.writeHead(200, { 'Content-Type': 'text/event-stream' })
  for (const event of [
    {
      type: 'message_start',
      message: {
        id: 'opus-fixture',
        type: 'message',
        role: 'assistant',
        model: body.model,
        content: [],
        usage: { input_tokens: 10, output_tokens: 0 },
      },
    },
    { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } },
    {
      type: 'content_block_delta',
      index: 0,
      delta: { type: 'text_delta', text: 'OPUS_55_ADAPTIVE_OK' },
    },
    { type: 'content_block_stop', index: 0 },
    {
      type: 'message_delta',
      delta: { stop_reason: 'end_turn', stop_sequence: null },
      usage: { output_tokens: 5 },
    },
    { type: 'message_stop' },
  ])
    response.write(`event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`)
  response.end()
})

async function start() {
  let ready = ''
  let diagnostics = ''
  child = spawn(executable, [], {
    cwd: workspace,
    env: {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([key]) =>
          /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP)$/i.test(key),
        ),
      ),
      PISPER_AGENT_DIR: agent,
      PI_CODING_AGENT_DIR: agent,
      PISPER_RS_DATA_DIR: agent,
      PISPER_WORKSPACE_DIR: workspace,
      PISPER_APP_ROOT: root,
      PISPER_DESKTOP_TOKEN: token,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PI_SKIP_VERSION_CHECK: '1',
      PI_TELEMETRY: '0',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  child.stderr.on('data', (chunk) => {
    diagnostics = (diagnostics + chunk).slice(-4000)
  })
  ended = new Promise((resolveExit) => child.once('exit', resolveExit))
  let failure
  child.on('error', (error) => {
    failure = error
  })
  child.stdout.on('data', (chunk) => {
    ready += chunk
    const line = ready.split('\n').find((value) => value.startsWith('PISPER_SIDECAR_READY '))
    if (line) base = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length)).url
  })
  const deadline = Date.now() + 30000
  while (!base && child.exitCode === null && !failure && Date.now() < deadline) await delay(50)
  assert.ok(
    base && !failure,
    `Isolated upgrade process must start: ${diagnostics.replaceAll(token, '<token>').replaceAll(primary, '<key>').replaceAll(secondary, '<key>').replaceAll(sandbox, '<fixture>').replaceAll(root, '<repository>')}`,
  )
}
async function stop() {
  if (child && child.exitCode === null) {
    child.stdin.end('shutdown\n')
    const timeout = setTimeout(() => child.kill('SIGKILL'), 15000)
    try {
      await ended
    } finally {
      clearTimeout(timeout)
    }
  }
  base = undefined
}
async function json(path, method = 'GET', body) {
  const response = await fetch(base + path, {
    method,
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(20000),
  })
  assert.equal(response.ok, true, `${method} ${path}: ${response.status}`)
  return response.json()
}
async function verify() {
  const config = await json('/api/config')
  const provider = config.providers.find((p) => p.id === 'upgrade-fixture')
  assert.deepEqual(
    provider.models.map((m) => m.id),
    ['claude-opus-5-5', 'manual-model'],
  )
  assert.equal(provider.apiKeys.length, 2)
  const opus = provider.models.find((m) => m.id === 'claude-opus-5-5')
  assert.ok(opus.thinkingLevels.includes('max'))
  assert.ok(!opus.thinkingLevels.includes('off'))
  assert.ok(!JSON.stringify(config).includes(primary))
  assert.ok(!JSON.stringify(config).includes(secondary))
  assert.equal(config.defaultModel, 'claude-opus-5-5')
  const exported = await json('/api/providers/export')
  assert.equal(exported.providerKeyrings.providers['upgrade-fixture'].length, 2)
  assert.equal(exported.providerModels.providers['upgrade-fixture'].models[0].id, 'claude-opus-5-5')
  return exported
}
try {
  await new Promise((resolve) => upstream.listen(0, '127.0.0.1', resolve))
  const modelBase = `http://127.0.0.1:${upstream.address().port}/v1`
  await Promise.all([agent, workspace].map((path) => mkdir(path)))
  const model = (id, extra = {}) => ({
    id,
    name: id,
    reasoning: true,
    input: ['text'],
    contextWindow: 32000,
    maxTokens: 1000,
    ...extra,
  })
  const documents = {
    'models.json': {
      providers: {
        'upgrade-fixture': {
          api: 'anthropic-messages',
          baseUrl: modelBase,
          models: [model('stale-model'), model('manual-model', { userConfigured: true })],
        },
      },
    },
    'auth.json': { 'upgrade-fixture': { type: 'api_key', key: primary } },
    'settings.json': { defaultProvider: 'upgrade-fixture', defaultModel: 'claude-opus-5-5' },
    'pisper.json': {},
    'pisper-provider-models.json': {
      providers: {
        'upgrade-fixture': {
          baseUrl: modelBase,
          api: 'anthropic-messages',
          models: [{ id: 'claude-opus-5-5', kind: 'chat' }],
        },
      },
    },
    'pisper-provider-keyrings.json': {
      version: 1,
      providers: { 'upgrade-fixture': [primary, secondary].map((key) => ({ id: id(key), key })) },
      future: true,
    },
  }
  for (const [name, value] of Object.entries(documents))
    await writeFile(join(agent, name), JSON.stringify(value))
  const originalCatalog = await readFile(join(agent, 'pisper-provider-models.json'), 'utf8')
  await start()
  const exported = await verify()
  const chat = await json('/api/sessions', 'POST', { name: 'Opus 5.5 protocol fixture' })
  assert.equal(chat.model, 'upgrade-fixture/claude-opus-5-5')
  await json(`/api/sessions/${chat.id}/thinking-level`, 'PUT', { level: 'high' })
  const bound = await json(`/api/sessions/${chat.id}/model`)
  assert.equal(bound.provider, 'upgrade-fixture')
  assert.equal(bound.id, 'claude-opus-5-5')
  const streamed = await fetch(base + '/api/chat', {
    method: 'POST',
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ sessionId: chat.id, message: 'Protocol validation fixture' }),
    signal: AbortSignal.timeout(30000),
  })
  const streamText = await streamed.text()
  assert.equal(
    streamed.ok,
    true,
    `Opus fixture HTTP ${streamed.status}: ${streamText.replaceAll(primary, '<key>').replaceAll(secondary, '<key>').replaceAll(sandbox, '<fixture>')}`,
  )
  assert.ok(
    streamText.includes('OPUS_55_ADAPTIVE_OK'),
    'Opus fixture must complete with a streamed reply',
  )
  assert(
    modelRequests.some(
      (request) => request.model === 'claude-opus-5-5' && request.output_config.effort === 'high',
    ),
  )
  const logs = await readdir(join(agent, 'sessions'), { recursive: true })
  let checkedHistory = false
  for (const name of logs.filter((name) => name.endsWith('.jsonl'))) {
    const entries = (await readFile(join(agent, 'sessions', name), 'utf8'))
      .trim()
      .split('\n')
      .map(JSON.parse)
    if (entries[0].id !== chat.id) continue
    const bindings = entries.filter((entry) => entry.type === 'model_change')
    assert(bindings.length > 0)
    assert(
      bindings.every(
        (entry) => entry.provider === 'upgrade-fixture' && entry.modelId === 'claude-opus-5-5',
      ),
      'SDK initialization must not append a fallback model to history',
    )
    checkedHistory = true
  }
  assert(checkedHistory)
  await json('/api/providers/upgrade-fixture/enabled', 'PUT', { enabled: false })
  const disabled = await json('/api/config')
  assert.deepEqual(
    disabled.providers.find((p) => p.id === 'upgrade-fixture').models.map((m) => m.id),
    ['claude-opus-5-5', 'manual-model'],
  )
  await stop()
  await start()
  const requestCount = modelRequests.length
  const unavailable = await fetch(base + '/api/chat', {
    method: 'POST',
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ sessionId: chat.id, message: 'Disabled model fixture' }),
    signal: AbortSignal.timeout(20000),
  })
  assert.equal(unavailable.status, 400)
  assert.equal((await unavailable.json()).code, 'model_not_available')
  assert.equal(modelRequests.length, requestCount, 'Disabled provider must never receive a request')
  assert.equal((await json(`/api/sessions/${chat.id}/model`)).id, 'claude-opus-5-5')
  await json('/api/providers/upgrade-fixture/enabled', 'PUT', { enabled: true })
  await json('/api/providers/import', 'POST', exported)
  await verify()
  await stop()
  await start()
  await verify()
  assert.equal((await json(`/api/sessions/${chat.id}/model`)).id, 'claude-opus-5-5')
  assert.equal(await readFile(join(agent, 'pisper-provider-models.json'), 'utf8'), originalCatalog)
  assert.equal(
    JSON.parse(await readFile(join(agent, 'pisper-provider-keyrings.json'))).future,
    true,
  )
  await json('/api/providers/upgrade-fixture/connection', 'PUT', {
    apiKeys: ['synthetic-replacement-a', 'synthetic-replacement-b'],
  })
  await json('/api/providers/upgrade-fixture/clone', 'POST', {
    id: 'clone-fixture',
    name: 'Clone fixture',
  })
  const cloned = await json('/api/providers/export')
  assert.equal(cloned.providerKeyrings.providers['clone-fixture'].length, 2)
  assert.equal(cloned.providerModels.providers['clone-fixture'].models[0].id, 'claude-opus-5-5')
  await json('/api/providers/clone-fixture', 'DELETE')
  const deleted = await json('/api/providers/export')
  assert.equal(deleted.providerKeyrings.providers['clone-fixture'], undefined)
  assert.equal(deleted.providerModels.providers['clone-fixture'], undefined)
  const replaced = await json('/api/providers/export')
  assert.deepEqual(
    replaced.providerKeyrings.providers['upgrade-fixture'].map((item) => item.key),
    ['synthetic-replacement-a', 'synthetic-replacement-b'],
  )
  await json('/api/providers/upgrade-fixture/connection', 'PUT', { clearApiKey: true })
  const cleared = await json('/api/providers/export')
  assert.equal(cleared.credentials['upgrade-fixture'], undefined)
  assert.equal(cleared.providerKeyrings.providers['upgrade-fixture'], undefined)
  console.log(
    'PASS provider upgrade: catalog, manual models, adaptive thinking, masked keys, export/import, restart, replacement and clear',
  )
} finally {
  await stop()
  await new Promise((resolve) => upstream.close(resolve))
  await rm(sandbox, { recursive: true, force: true })
}
