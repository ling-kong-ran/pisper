// 使用真实聊天入口和本机模型夹具，检查项目规则到供应商请求的完整链路。
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { resolve, join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(process.argv[2] || join(root, 'runtime-rs/target/debug/pisper-server'))
const output = join(root, 'release/project-context-verification')
await mkdir(output, { recursive: true })
const sandbox = await mkdtemp(join(output, 'run-'))
const agent = join(sandbox, 'agent')
const workspace = join(sandbox, 'project')
const second = join(sandbox, 'second-project')
await Promise.all([agent, workspace, second].map((dir) => mkdir(dir)))
const marker = 'PISPER_AGENTS_CONTEXT_PROBE_20261010'
const secondMarker = 'PISPER_SECOND_PROJECT_CONTEXT_PROBE_20261010'
await writeFile(
  join(workspace, 'AGENTS.md'),
  `# Project instructions\n\nWhen answering CONTEXT_PROBE requests, include the marker ${marker}.\n`,
)
await writeFile(
  join(second, 'AGENTS.md'),
  `# Project instructions\n\nWhen answering CONTEXT_PROBE requests, include the marker ${secondMarker}.\n`,
)
const requests = []
const model = createServer(async (req, res) => {
  try {
    let raw = ''
    for await (const chunk of req) {
      raw += chunk
      assert.ok(raw.length <= 8 * 1024 * 1024)
    }
    const body = JSON.parse(raw)
    const anthropic = new URL(req.url, 'http://127.0.0.1').pathname.endsWith('/messages')
    const system = anthropic
      ? JSON.stringify(body.system)
      : JSON.stringify(
          (body.messages || []).filter((message) => ['system', 'developer'].includes(message.role)),
        )
    const current = JSON.stringify(body).includes('CONTEXT_PROBE_SECOND') ? secondMarker : marker
    requests.push({
      protocol: anthropic ? 'anthropic-messages' : 'openai-completions',
      model: body.model,
      expectedMarker: current,
      systemContainsMarker: system.includes(current),
      requestContainsMarker: JSON.stringify(body).includes(current),
    })
    // 只保存本机夹具请求的系统部分；请求不发送到外部服务，也不使用真实凭据。
    await writeFile(
      join(sandbox, `request-${requests.length}-system.json`),
      JSON.stringify(
        anthropic
          ? body.system
          : body.messages.filter((message) => ['system', 'developer'].includes(message.role)),
        null,
        2,
      ),
    )
    res.writeHead(200, { 'Content-Type': 'text/event-stream' })
    if (anthropic) {
      const emit = (type, data) =>
        res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`)
      emit('message_start', {
        message: {
          id: 'fixture',
          type: 'message',
          role: 'assistant',
          model: body.model,
          content: [],
          stop_reason: null,
          stop_sequence: null,
          usage: { input_tokens: 1, output_tokens: 0 },
        },
      })
      emit('content_block_start', { index: 0, content_block: { type: 'text', text: '' } })
      emit('content_block_delta', { index: 0, delta: { type: 'text_delta', text: 'CONTEXT_OK' } })
      emit('content_block_stop', { index: 0 })
      emit('message_delta', {
        delta: { stop_reason: 'end_turn', stop_sequence: null },
        usage: { output_tokens: 3 },
      })
      emit('message_stop', {})
    } else {
      for (const chunk of [{ role: 'assistant', content: 'CONTEXT_OK' }, {}]) {
        res.write(
          `data: ${JSON.stringify({ id: 'fixture', object: 'chat.completion.chunk', created: 1, model: body.model, choices: [{ index: 0, delta: chunk, finish_reason: chunk.content ? null : 'stop' }] })}\n\n`,
        )
      }
      res.write('data: [DONE]\n\n')
    }
    res.end()
  } catch (error) {
    await writeFile(join(sandbox, 'fixture-error.txt'), String(error))
    if (!res.headersSent) res.writeHead(500)
    res.end('Fixture request failed')
  }
})
await new Promise((resolveListen) => model.listen(0, '127.0.0.1', resolveListen))
const modelBase = `http://127.0.0.1:${model.address().port}`
const providers = {
  'claude-fixture': {
    api: 'anthropic-messages',
    baseUrl: modelBase,
    models: [
      {
        id: 'claude-opus-5-5',
        name: 'Claude fixture',
        reasoning: false,
        input: ['text'],
        contextWindow: 128000,
        maxTokens: 1000,
      },
    ],
  },
  'glm-fixture': {
    api: 'openai-completions',
    baseUrl: modelBase + '/v1',
    models: [
      {
        id: 'glm-fixture',
        name: 'GLM protocol fixture',
        reasoning: false,
        input: ['text'],
        contextWindow: 128000,
        maxTokens: 1000,
      },
    ],
  },
}
for (const [name, data] of Object.entries({
  'models.json': { providers },
  'auth.json': Object.fromEntries(
    Object.keys(providers).map((id) => [id, { type: 'api_key', key: 'synthetic-context-key' }]),
  ),
  'settings.json': { defaultProvider: 'glm-fixture', defaultModel: 'glm-fixture' },
  'pisper.json': {},
}))
  await writeFile(join(agent, name), JSON.stringify(data), { mode: 0o600 })
const token = 'synthetic_project_context_token'
const env = Object.fromEntries(
  Object.entries(process.env).filter(([key]) =>
    /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP)$/i.test(key),
  ),
)
Object.assign(env, {
  PISPER_AGENT_DIR: agent,
  PI_CODING_AGENT_DIR: agent,
  PISPER_RS_DATA_DIR: agent,
  PISPER_WORKSPACE_DIR: workspace,
  PISPER_APP_ROOT: root,
  PISPER_DESKTOP_TOKEN: token,
  PISPER_EXIT_ON_STDIN_CLOSE: '1',
  PI_SKIP_VERSION_CHECK: '1',
  PI_TELEMETRY: '0',
})
const child = spawn(executable, [], { cwd: workspace, env, stdio: ['pipe', 'pipe', 'pipe'] })
const ended = new Promise((resolveExit) => child.once('exit', resolveExit))
let base,
  captured = '',
  failure
child.on('error', (error) => {
  failure = error
})
child.stderr.on('data', () => {})
child.stdout.on('data', (chunk) => {
  captured += chunk
  const line = captured.split('\n').find((line) => line.startsWith('PISPER_SIDECAR_READY '))
  if (line) base = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length)).url
})
async function api(path, method = 'GET', body) {
  const response = await fetch(base + path, {
    method,
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(30000),
  })
  assert.ok(response.ok, `${method} ${path}: ${response.status}`)
  return response
}
try {
  const deadline = Date.now() + 30000
  while (!base && child.exitCode === null && !failure && Date.now() < deadline) await delay(50)
  assert.ok(base, 'Context fixture backend must start')
  const session = await (
    await api('/api/sessions', 'POST', { name: 'AGENTS context verification', cwd: workspace })
  ).json()
  for (const [directory, label] of [
    [workspace, 'FIRST'],
    [second, 'SECOND'],
  ]) {
    if (directory !== workspace)
      await api(`/api/sessions/${session.id}/cwd`, 'PUT', { cwd: directory })
    for (const [provider, modelId] of [
      ['claude-fixture', 'claude-opus-5-5'],
      ['glm-fixture', 'glm-fixture'],
    ]) {
      await api(`/api/sessions/${session.id}/model`, 'POST', { provider, model: modelId })
      const before = requests.length
      const response = await api('/api/chat', 'POST', {
        sessionId: session.id,
        message: `CONTEXT_PROBE_${label}`,
        attachments: [],
        goalMode: false,
        teamMode: false,
      })
      const stream = await response.text()
      await writeFile(join(sandbox, `${label}-${provider}.sse`), stream)
      assert.ok(stream.includes('CONTEXT_OK'), 'Chat must receive the fixture reply')
      assert.ok(requests.length > before, 'A real model HTTP request must be observed')
      const observed = requests.slice(before)
      assert.ok(
        observed.every((request) => request.requestContainsMarker),
        'Project instructions must reach the model request',
      )
      if (label === 'FIRST')
        assert.ok(
          observed.every((request) => request.systemContainsMarker),
          'Initial project rules must be in system context',
        )
    }
  }
  const report = {
    passed: true,
    agentsFile: join(workspace, 'AGENTS.md'),
    marker,
    requests,
    limitation:
      'Checks instruction delivery using local model fixtures, not remote-model obedience.',
  }
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  console.log(JSON.stringify(report, null, 2))
} finally {
  if (child.exitCode === null) child.stdin.end('shutdown\n')
  const timeout = setTimeout(() => child.kill('SIGKILL'), 15000)
  try {
    await ended
  } finally {
    clearTimeout(timeout)
  }
  await new Promise((resolveClose) => model.close(resolveClose))
}
