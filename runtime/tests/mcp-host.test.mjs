import assert from 'node:assert/strict'
import { stat, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { request } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import test from 'node:test'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import { McpHostService } from '../services/mcp-host-service.mjs'
import { mcpHostRoutes } from '../http/routes/mcp-host.mjs'

function mockRuntime() {
  const sessions = []
  const prompts = []
  return {
    capabilities: { profile: 'desktop', features: { chat: true } },
    sessions: new Map(),
    pendingSessions: new Map(),
    cwd: tmpdir(),
    async listSessions() {
      return [...sessions]
    },
    async createSession(name = 'New conversation', cwd = tmpdir()) {
      const session = { id: `session-${sessions.length + 1}`, name, cwd }
      sessions.push(session)
      this.pendingSessions.set(session.id, session)
      return session
    },
    async findSessionInfo(id) {
      return sessions.find((session) => session.id === id) || null
    },
    async hasSession(id) {
      return Boolean((await this.findSessionInfo(id)) || this.pendingSessions.has(id))
    },
    async renameSession(id, name) {
      const session = sessions.find((entry) => entry.id === id)
      if (session) session.name = name
      return session || null
    },
    async getSessionMessagePage(id) {
      return {
        messages: [{ id: 'm1', role: 'agent', text: `Hello from ${id}` }],
        pageInfo: { nextCursor: null, hasMore: false },
        model: 'example/model',
      }
    },
    async getSessionLive(id) {
      return { sessionId: id, streaming: false, approvals: [], messages: [] }
    },
    async promptFromChannel({ sessionId, message, onEvent }) {
      prompts.push({ sessionId, message })
      onEvent?.('text_delta', { delta: 'Hello' })
      await delay(5)
      return { sessionId, text: `Hello: ${message}` }
    },
    async abortSession(id) {
      prompts.push({ aborted: id })
      return true
    },
    getSessionGoal: () => null,
    pauseSessionGoal: async () => null,
    searchMemory: async (query) => [{ id: 'memory-1', title: query }],
    getWorkflows: async () => ({ workflows: [] }),
    runWorkflow: async () => ({ started: true }),
    getWorkflowRun: () => null,
    stopWorkflowRun: async () => null,
    getSchedules: async () => ({ tasks: [] }),
    runSchedule: async () => ({ started: true }),
    getTodayUsage: async () => ({ tokens: 5 }),
    listAssets: async () => [],
    getSessionFileChanges: async () => ({ files: [] }),
    prompts,
  }
}

async function connect(credentials) {
  const client = new Client({ name: 'pisper-test', version: '1.0.0' })
  const transport = new StreamableHTTPClientTransport(new URL(credentials.url), {
    requestInit: { headers: { Authorization: `Bearer ${credentials.token}` } },
  })
  await client.connect(transport)
  return { client, transport }
}

function toolResult(value) {
  return JSON.parse(value.content.find((item) => item.type === 'text').text)
}

function requestStatus(url, headers) {
  return new Promise((resolveStatus, rejectStatus) => {
    const req = request(url, { method: 'POST', headers }, (res) => {
      res.resume()
      res.on('end', () => resolveStatus(res.statusCode))
    })
    req.on('error', rejectStatus)
    req.end()
  })
}

test('MCP host remains closed by default, persists opt-in, and authenticates SDK clients', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mcp-host-'))
  const runtime = mockRuntime()
  let host = new McpHostService({ dataDir, getRuntime: () => runtime, port: 0 })
  t.after(async () => {
    await host.close()
    await rm(dataDir, { recursive: true, force: true })
  })
  assert.deepEqual(host.getCredentials(), null)
  assert.equal((await host.startIfEnabled()).listening, false)
  assert.equal(host.status().enabled, false)

  const enabled = await host.setEnabled(true)
  assert.equal(enabled.enabled, true)
  assert.equal(enabled.listening, true)
  assert.match(enabled.url, /^http:\/\/127\.0\.0\.1:\d+\/mcp$/)
  assert.doesNotMatch(JSON.stringify(enabled), /[A-Za-z0-9_-]{43}/)
  if (process.platform !== 'win32') {
    const mode = (await stat(join(dataDir, 'pisper-mcp-host.json'))).mode & 0o777
    assert.equal(mode, 0o600)
  }

  const credentials = host.getCredentials()
  const unauthenticated = await fetch(credentials.url, { method: 'POST' })
  assert.equal(unauthenticated.status, 401)
  const forgedHost = await requestStatus(credentials.url, {
    Authorization: `Bearer ${credentials.token}`,
    Host: 'evil.example',
  })
  assert.equal(forgedHost, 403)
  const forgedOrigin = await fetch(credentials.url, {
    method: 'POST',
    headers: { Authorization: `Bearer ${credentials.token}`, Origin: 'https://evil.example' },
  })
  assert.equal(forgedOrigin.status, 403)
  const { client, transport } = await connect(credentials)
  const tools = await client.listTools()
  assert.ok(tools.tools.some((tool) => tool.name === 'pisper_send_message'))
  assert.ok(tools.tools.some((tool) => tool.name === 'pisper_search_memory'))
  assert.equal(
    tools.tools.some((tool) => tool.name === 'pisper_git_push'),
    false,
  )

  const created = toolResult(
    await client.callTool({ name: 'pisper_create_session', arguments: { name: 'External task' } }),
  )
  assert.equal(created.name, 'External task')
  const listed = toolResult(
    await client.callTool({ name: 'pisper_list_sessions', arguments: { limit: 10 } }),
  )
  assert.equal(listed.sessions[0].id, created.id)
  runtime.getSessionMessagePage = async () => ({
    messages: [
      {
        id: 'secret-message',
        role: 'agent',
        text: 'postgres://user:password@host/db',
      },
    ],
    pageInfo: { nextCursor: null, hasMore: false },
  })
  const page = toolResult(
    await client.callTool({
      name: 'pisper_get_session_messages',
      arguments: { sessionId: created.id },
    }),
  )
  assert.equal(page.messages[0].text, '[REDACTED SECRET]')
  runtime.getSessionMessagePage = async () => ({
    messages: [{ id: 'broken', role: 'agent', text: '\ud800' }],
    pageInfo: { nextCursor: null, hasMore: false },
  })
  const safePage = toolResult(
    await client.callTool({
      name: 'pisper_get_session_messages',
      arguments: { sessionId: created.id },
    }),
  )
  assert.equal(safePage.messages[0].text, '\ufffd')
  const missingWorkflow = await client.callTool({
    name: 'pisper_get_workflow_run',
    arguments: { runId: 'missing' },
  })
  assert.equal(missingWorkflow.isError, true)
  assert.equal(toolResult(missingWorkflow).error, 'not_found')
  const start = toolResult(
    await client.callTool({
      name: 'pisper_send_message',
      arguments: { sessionId: created.id, message: 'Can you help?' },
    }),
  )
  assert.equal(start.status, 'running')
  let run
  for (let attempt = 0; attempt < 30; attempt += 1) {
    run = toolResult(
      await client.callTool({ name: 'pisper_get_run', arguments: { runId: start.id } }),
    )
    if (run.status !== 'running') break
    await delay(10)
  }
  assert.equal(run.status, 'completed')
  assert.equal(run.text, 'Hello: Can you help?')
  assert.deepEqual(runtime.prompts[0], { sessionId: created.id, message: 'Can you help?' })
  await client.close()
  await transport.close().catch(() => {})

  const rotated = await host.rotateToken()
  assert.notEqual(rotated.token, credentials.token)
  const oldToken = await fetch(rotated.url, {
    method: 'POST',
    headers: { Authorization: `Bearer ${credentials.token}` },
  })
  assert.equal(oldToken.status, 401)
  await host.close()
  host = new McpHostService({ dataDir, getRuntime: () => runtime, port: 0 })
  const restored = await host.startIfEnabled()
  assert.equal(restored.enabled, true)
  assert.equal(restored.listening, true)
  assert.equal(host.getCredentials().token, rotated.token)
  const disabled = await host.setEnabled(false)
  assert.equal(disabled.listening, false)
  assert.equal(host.getCredentials(), null)
  await host.close()

  host = new McpHostService({ dataDir, getRuntime: () => runtime, port: 0 })
  assert.equal((await host.startIfEnabled()).listening, false)
  assert.equal(host.status().enabled, false)
})

test('MCP host bounds request size and does not expose token in status', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mcp-boundary-'))
  const host = new McpHostService({ dataDir, getRuntime: () => mockRuntime(), port: 0 })
  let conflicting
  t.after(async () => {
    await conflicting?.close()
    await host.close()
    await rm(dataDir, { recursive: true, force: true })
  })
  await host.setEnabled(true)
  const credentials = host.getCredentials()
  assert.equal(JSON.stringify(host.status()).includes(credentials.token), false)
  const tooLarge = await fetch(credentials.url, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${credentials.token}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ value: 'x'.repeat(256 * 1024) }),
  })
  assert.equal(tooLarge.status, 413)

  conflicting = new McpHostService({
    dataDir: join(dataDir, 'second'),
    getRuntime: () => mockRuntime(),
    port: host.status().port,
  })
  const conflict = await conflicting.setEnabled(true)
  assert.equal(conflict.enabled, true)
  assert.equal(conflict.listening, false)
  assert.match(conflict.error, /端口已被占用/)
  assert.equal(conflicting.getCredentials(), null)
})

test('MCP host limits concurrent agent turns and revokes active turns when disabled', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mcp-runs-'))
  const runtime = mockRuntime()
  runtime.promptFromChannel = () => new Promise(() => {})
  const host = new McpHostService({ dataDir, getRuntime: () => runtime, port: 0 })
  t.after(async () => {
    await host.close()
    await rm(dataDir, { recursive: true, force: true })
  })
  await host.setEnabled(true)
  const { client, transport } = await connect(host.getCredentials())
  for (let index = 0; index < 4; index += 1) {
    const started = await client.callTool({
      name: 'pisper_send_message',
      arguments: { message: `Request ${index}` },
    })
    assert.equal(toolResult(started).status, 'running')
  }
  const rejected = await client.callTool({
    name: 'pisper_send_message',
    arguments: { message: 'One too many' },
  })
  assert.equal(rejected.isError, true)
  assert.match(toolResult(rejected).message, /上限/)
  await client.close()
  await transport.close().catch(() => {})
  await host.setEnabled(false)
  assert.equal(runtime.prompts.filter((item) => item.aborted).length, 4)
  await host.setEnabled(true)
  const reopened = await connect(host.getCredentials())
  const next = await reopened.client.callTool({
    name: 'pisper_send_message',
    arguments: { message: 'After restart' },
  })
  assert.equal(toolResult(next).status, 'running')
  await reopened.client.close()
  await reopened.transport.close().catch(() => {})
})

test('MCP host management is limited to local app requests, including credential reads', async () => {
  const calls = []
  const services = {
    mcpHost: {
      status: () => ({ enabled: true, listening: true }),
      setEnabled: async () => ({ enabled: false }),
      getCredentials: () => ({ token: 'private' }),
      rotateToken: async () => ({ token: 'rotated' }),
    },
  }
  const invoke = async (route, overrides = {}) => {
    let response
    await route.handler({
      services,
      req: {
        socket: { remoteAddress: '127.0.0.1' },
        headers: { host: '127.0.0.1:5173' },
        ...overrides,
      },
      body: async () => ({ enabled: false }),
      json: (status, value) => {
        response = { status, value }
        calls.push(response)
      },
    })
    return response
  }
  for (const route of mcpHostRoutes) {
    const remote = await invoke(route, { pisperRemote: true })
    assert.equal(remote.status, 403)
    assert.equal(remote.value.code, 'mcp_host_local_only')
    const forgedHost = await invoke(route, { headers: { host: 'evil.example' } })
    assert.equal(forgedHost.status, 403)
    const forgedOrigin = await invoke(route, {
      headers: { host: '127.0.0.1:5173', origin: 'https://evil.example' },
    })
    assert.equal(forgedOrigin.status, 403)
  }
  assert.equal((await invoke(mcpHostRoutes[0])).status, 200)
  assert.deepEqual((await invoke(mcpHostRoutes[2])).value, { token: 'private' })
  assert.equal(calls.filter(({ value }) => value?.token).length, 1)
})

test('MCP run reports a stream error event as failure', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mcp-stream-error-'))
  const runtime = mockRuntime()
  runtime.promptFromChannel = async ({ onEvent }) => {
    onEvent('error', { message: 'Model unavailable' })
    return { text: '' }
  }
  const host = new McpHostService({ dataDir, getRuntime: () => runtime, port: 0 })
  t.after(async () => {
    await host.close()
    await rm(dataDir, { recursive: true, force: true })
  })
  await host.setEnabled(true)
  const { client, transport } = await connect(host.getCredentials())
  t.after(async () => {
    await client.close()
    await transport.close().catch(() => {})
  })
  const start = toolResult(
    await client.callTool({ name: 'pisper_send_message', arguments: { message: 'Hi' } }),
  )
  let run
  for (let attempt = 0; attempt < 30; attempt += 1) {
    run = toolResult(
      await client.callTool({ name: 'pisper_get_run', arguments: { runId: start.id } }),
    )
    if (run.status !== 'running') break
    await delay(10)
  }
  assert.equal(run.status, 'failed')
  assert.equal(run.error, 'Model unavailable')
})

test('MCP run waits for Pisper approval instead of approving external tool calls', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mcp-approval-'))
  const runtime = mockRuntime()
  let resolveApproval
  runtime.promptFromChannel = async ({ onEvent }) => {
    onEvent('permission_request', { id: 'approval-1' })
    await new Promise((resolve) => {
      resolveApproval = resolve
    })
    onEvent('permission_resolved', { id: 'approval-1' })
    return { text: 'Approved by the user' }
  }
  const host = new McpHostService({ dataDir, getRuntime: () => runtime, port: 0 })
  t.after(async () => {
    resolveApproval?.()
    await host.close()
    await rm(dataDir, { recursive: true, force: true })
  })
  await host.setEnabled(true)
  const { client, transport } = await connect(host.getCredentials())
  t.after(async () => {
    await client.close()
    await transport.close().catch(() => {})
  })
  const start = toolResult(
    await client.callTool({ name: 'pisper_send_message', arguments: { message: 'Run a tool' } }),
  )
  let pending
  for (let attempt = 0; attempt < 30; attempt += 1) {
    pending = toolResult(
      await client.callTool({ name: 'pisper_get_run', arguments: { runId: start.id } }),
    )
    if (pending.needsApproval) break
    await delay(10)
  }
  assert.equal(pending.status, 'running')
  assert.equal(pending.needsApproval, true)
  resolveApproval()
  let completed
  for (let attempt = 0; attempt < 30; attempt += 1) {
    completed = toolResult(
      await client.callTool({ name: 'pisper_get_run', arguments: { runId: start.id } }),
    )
    if (completed.status !== 'running') break
    await delay(10)
  }
  assert.equal(completed.status, 'completed')
  assert.equal(completed.needsApproval, false)
})

test('invalid optional MCP settings do not prevent Pisper startup', async (t) => {
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mcp-invalid-'))
  await writeFile(join(dataDir, 'pisper-mcp-host.json'), '{ invalid', 'utf8')
  const host = new McpHostService({ dataDir, getRuntime: () => mockRuntime(), port: 0 })
  t.after(async () => {
    await host.close()
    await rm(dataDir, { recursive: true, force: true })
  })
  const status = await host.startIfEnabled()
  assert.equal(status.enabled, false)
  assert.equal(status.listening, false)
  assert.match(status.error, /设置读取失败/)
  assert.equal((await host.setEnabled(true)).listening, true)
})
