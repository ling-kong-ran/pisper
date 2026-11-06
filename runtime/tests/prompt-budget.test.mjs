// 真实 sidecar + 假模型：看真正发给模型的请求。系统提示词和工具定义每次请求都要发，
// 这里防止它们悄悄涨回去，也防止每轮都变的内容混进前缀，让对话历史命中不了提示词缓存。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readdir, readFile, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { estimateTextTokens } from '../runtime/context-breakdown.mjs'

const appRoot = resolve(process.env.PISPER_TEST_APP_ROOT || '.')
// 不是目标，是防回涨。确实要加内容就调高，并在变更说明里写清楚为什么值得每次请求都发。
const SYSTEM_PROMPT_BUDGET = 1250
const TOOL_DEFINITIONS_BUDGET = 2100

function fakeModel() {
  const requests = []
  const script = []
  const childScript = []
  const childRequests = []
  const server = createServer(async (req, res) => {
    if (req.url?.endsWith('/models')) {
      res.setHeader('content-type', 'application/json')
      res.end(JSON.stringify({ data: [{ id: 'fake-model', object: 'model' }] }))
      return
    }
    let body = ''
    for await (const chunk of req) body += chunk
    const parsed = JSON.parse(body || '{}')
    if (!parsed.stream) {
      res.setHeader('content-type', 'application/json')
      res.end(
        JSON.stringify({
          id: 'x',
          object: 'chat.completion',
          created: 1,
          model: 'fake-model',
          choices: [
            { index: 0, message: { role: 'assistant', content: '{}' }, finish_reason: 'stop' },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      )
      return
    }
    const child = JSON.stringify(parsed.messages[0]).includes('You are a Pisper subagent')
    ;(child ? childRequests : requests).push(parsed)
    const chunk = (delta, finish = null) =>
      res.write(
        `data: ${JSON.stringify({ id: 'x', object: 'chat.completion.chunk', created: 1, model: 'fake-model', choices: [{ index: 0, delta, finish_reason: finish }] })}\n\n`,
      )
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const next = (child ? childScript : script).shift()
    if (next) {
      chunk({
        role: 'assistant',
        tool_calls: [
          {
            index: 0,
            id: `call_${requests.length}`,
            type: 'function',
            function: { name: next.name, arguments: JSON.stringify(next.arguments) },
          },
        ],
      })
      chunk({}, 'tool_calls')
    } else {
      chunk({ role: 'assistant', content: 'ok' })
      chunk({}, 'stop')
    }
    res.write('data: [DONE]\n\n')
    res.end()
  })
  return { server, requests, script, childScript, childRequests }
}

async function start(t) {
  const model = fakeModel()
  await new Promise((done) => model.server.listen(0, '127.0.0.1', done))
  const home = await mkdtemp(join(tmpdir(), 'pisper-prompt-'))
  const dataDir = join(home, 'data')
  const project = join(home, 'project')
  await mkdir(dataDir)
  await mkdir(project)
  const child = spawn(process.execPath, [join(appRoot, 'runtime', 'sidecar.mjs')], {
    cwd: appRoot,
    env: {
      ...process.env,
      HOME: home,
      PI_OFFLINE: '1',
      PISPER_AGENT_DIR: dataDir,
      PISPER_DESKTOP_TOKEN: 'prompt-token',
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PISPER_WORKSPACE_DIR: project,
    },
    stdio: ['pipe', 'pipe', 'inherit'],
  })
  const ready = await new Promise((resolveReady, reject) => {
    let buffer = ''
    child.stdout.on('data', (chunk) => {
      buffer += chunk
      const match = buffer.match(/PISPER_SIDECAR_READY (.*)\n/)
      if (match) resolveReady(JSON.parse(match[1]))
    })
    child.once('exit', (code) => reject(new Error(`sidecar exited ${code}`)))
  })
  t.after(async () => {
    const exited = new Promise((resume) => child.once('exit', resume))
    child.stdin.write('shutdown\n')
    await Promise.race([exited, new Promise((resume) => setTimeout(resume, 8000))])
    if (child.exitCode === null) child.kill('SIGKILL')
    model.server.close()
    await rm(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 })
  })
  const origin = new URL(ready.url).origin
  const boot = await fetch(ready.bootstrapUrl, { redirect: 'manual' })
  const headers = {
    cookie: boot.headers.get('set-cookie').split(';')[0],
    origin,
    'content-type': 'application/json',
  }
  const api = async (method, path, body) => {
    const response = await fetch(`${origin}${path}`, {
      method,
      headers,
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    })
    const text = await response.text()
    try {
      return { status: response.status, data: JSON.parse(text) }
    } catch {
      return { status: response.status, data: text }
    }
  }
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if ((await api('GET', '/api/sessions')).status === 200) break
    await new Promise((resume) => setTimeout(resume, 100))
  }
  await api('POST', '/api/providers', {
    id: 'fake',
    name: 'Fake',
    api: 'openai-completions',
    baseUrl: `http://127.0.0.1:${model.server.address().port}/v1`,
    model: 'fake-model',
    apiKey: 'not-a-real-key',
    contextWindow: 200000,
  })
  const chat = async (sessionId, message) => {
    const response = await fetch(`${origin}/api/chat`, {
      method: 'POST',
      headers: { ...headers, accept: 'text/event-stream' },
      body: JSON.stringify({ sessionId, message }),
    })
    for await (const part of response.body) void part
  }
  return { api, chat, model, project, dataDir }
}

const systemText = (request) => {
  const heads = request.messages.filter((item) => ['system', 'developer'].includes(item.role))
  assert.equal(heads.length, 1, 'the request has one system prompt, at the head')
  assert.equal(request.messages[0], heads[0])
  return typeof heads[0].content === 'string' ? heads[0].content : JSON.stringify(heads[0].content)
}

test('the prompt stays within budget and a new message never changes what came before', async (t) => {
  const { api, chat, model, dataDir } = await start(t)
  const created = await api('POST', '/api/sessions', { name: 'budget' })
  const sessionId = created.data.id
  await chat(sessionId, 'first')
  // 跨过至少一分钟的取整边界不必等：时钟不在前缀里，换秒、换分都不该影响这里的比较。
  await new Promise((resume) => setTimeout(resume, 1100))
  await chat(sessionId, 'second')
  const [first, second] = model.requests

  const system = systemText(first)
  const systemTokens = estimateTextTokens(system)
  const toolTokens = first.tools.reduce(
    (sum, tool) => sum + estimateTextTokens(JSON.stringify(tool.function)),
    0,
  )
  assert.ok(systemTokens <= SYSTEM_PROMPT_BUDGET, `system prompt is ${systemTokens} tokens`)
  assert.ok(toolTokens <= TOOL_DEFINITIONS_BUDGET, `tool definitions are ${toolTokens} tokens`)

  // 前缀稳定：系统提示词、工具定义、已有消息一个字节都不变，新内容只追加在末尾。
  assert.deepEqual(second.tools, first.tools)
  assert.deepEqual(second.messages.slice(0, first.messages.length), first.messages)
  assert.doesNotMatch(system, /\d{4}-\d{2}-\d{2}/, 'no date in the system prompt')

  // Pi 的名字都换掉了，工具不再列第二遍，指向打包后不存在文件的文档指引也不在了。
  assert.doesNotMatch(system, /inside pi\b|Pi documentation/)
  assert.match(system, /inside Pisper/)
  assert.doesNotMatch(system, /<tools>|<docs>/)
  assert.match(system, /as untrusted data/)

  // 模型能从最新一条用户消息读到现在的时间，但这只是投影：转录里没有。
  const latest = second.messages.at(-1)
  assert.equal(latest.role, 'user')
  assert.match(JSON.stringify(latest.content), /<sent_at>\d{4}-\d{2}-\d{2} \d{2}:\d{2} /)
  assert.match(JSON.stringify(latest.content), /second/)
  const sessionsDir = join(dataDir, 'sessions')
  const files = await readdir(sessionsDir)
  const transcript = await readFile(join(sessionsDir, files[0]), 'utf8')
  assert.match(transcript, /second/)
  assert.doesNotMatch(transcript, /<sent_at>\d/)

  // 上下文面板按转录统计，转录里的段落要和真正发送的一致（假模型不报用量，这里是未校准的估算）。
  const breakdown = (await api('GET', `/api/sessions/${sessionId}/context-breakdown`)).data
  const part = (id) => breakdown.categories.find((item) => item.id === id).tokens
  const counted = part('system') + part('project') + part('skills')
  assert.ok(
    Math.abs(counted - systemTokens) / systemTokens < 0.03,
    `panel counts ${counted}, request has ${systemTokens}`,
  )
  assert.ok(
    Math.abs(part('tools') - toolTokens) / toolTokens < 0.05,
    `panel counts ${part('tools')}, request has ${toolTokens}`,
  )
})

test('the coding tools with lean text still run', async (t) => {
  const { api, chat, model, project } = await start(t)
  const created = await api('POST', '/api/sessions', { name: 'tools', cwd: project })
  const sessionId = created.data.id
  await api('PUT', `/api/sessions/${sessionId}/execution-mode`, { mode: 'full-access' })
  model.script.push(
    { name: 'write', arguments: { path: 'notes/a.txt', content: 'alpha\nkeep\n' } },
    { name: 'read', arguments: { path: 'notes/a.txt' } },
    {
      name: 'edit',
      arguments: { path: 'notes/a.txt', edits: [{ oldText: 'alpha', newText: 'beta' }] },
    },
    { name: 'grep', arguments: { pattern: 'beta', path: 'notes' } },
    { name: 'find', arguments: { pattern: '*.txt' } },
    { name: 'ls', arguments: { path: 'notes' } },
    { name: 'bash', arguments: { command: 'cat notes/a.txt' } },
  )
  await chat(sessionId, 'use the tools')
  assert.equal(await readFile(join(project, 'notes/a.txt'), 'utf8'), 'beta\nkeep\n')
  const results = model.requests
    .at(-1)
    .messages.filter((item) => item.role === 'tool')
    .map((item) => JSON.stringify(item.content))
  assert.equal(results.length, 7)
  assert.match(results[1], /alpha/, 'read')
  // grep、find 依赖 rg、fd，开发环境和 CI 可能没装（离线时也不会下载），打包后由 smoke-sea 覆盖。
  assert.match(results[3], /beta|ripgrep \(rg\) is not available/, 'grep')
  assert.match(results[4], /a\.txt|fd is not available/, 'find')
  assert.match(results[5], /a\.txt/, 'ls')
  assert.match(results[6], /beta/, 'bash')
  // 发给模型的确实是精简后的说明。
  const definitions = new Map(model.requests[0].tools.map((tool) => [tool.function.name, tool]))
  for (const name of ['read', 'edit', 'write', 'grep', 'find', 'ls', 'bash'])
    assert.ok(definitions.has(name), name)
  assert.doesNotMatch(JSON.stringify(definitions.get('read')), /whichever is hit first/)
  assert.doesNotMatch(JSON.stringify(definitions.get('edit')), /Do not include overlapping/)
})

test('subagents get the same lean tools and can run them', async (t) => {
  const { api, chat, model, project } = await start(t)
  const created = await api('POST', '/api/sessions', { name: 'child', cwd: project })
  const sessionId = created.data.id
  await api('PUT', `/api/sessions/${sessionId}/execution-mode`, { mode: 'full-access' })
  model.script.push({
    name: 'spawn_agent',
    arguments: { taskName: 'writer', message: 'Write child.txt with the word done.' },
  })
  model.childScript.push({ name: 'write', arguments: { path: 'child.txt', content: 'done\n' } })
  await chat(sessionId, 'delegate this')
  for (let attempt = 0; attempt < 100 && model.childRequests.length < 2; attempt += 1)
    await new Promise((resume) => setTimeout(resume, 100))
  assert.equal(await readFile(join(project, 'child.txt'), 'utf8'), 'done\n')
  const [first] = model.childRequests
  const system = systemText(first)
  assert.match(system, /You are a Pisper subagent/)
  assert.match(system, /inside Pisper/)
  assert.match(system, /as untrusted data/)
  assert.doesNotMatch(system, /<tools>|<docs>/)
  const definitions = new Map(first.tools.map((tool) => [tool.function.name, tool]))
  assert.equal(definitions.has('spawn_agent'), false, 'subagents cannot spawn')
  assert.doesNotMatch(JSON.stringify(definitions.get('read')), /whichever is hit first/)
  assert.doesNotMatch(JSON.stringify(definitions.get('edit')), /Do not include overlapping/)
  assert.match(JSON.stringify(first.messages), /<sent_at>/)
})
