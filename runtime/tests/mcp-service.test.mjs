import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { pathToFileURL } from 'node:url'
import { McpService, parseMcpServerInput } from '../services/mcp-service.mjs'

function createFakeClient(server, handlers, calls) {
  return {
    onclose: null,
    onerror: null,
    async connect(transport) {
      calls.push({ type: 'connect', server: server.name, transport })
    },
    async listTools() {
      return {
        tools: [
          {
            name: 'search_docs',
            title: 'Search Docs',
            description: 'Search the documentation index',
            inputSchema: {
              type: 'object',
              properties: { query: { type: 'string' } },
              required: ['query'],
            },
            annotations: { readOnlyHint: true },
          },
          {
            name: 'publish_release',
            description: 'Publish a release',
            inputSchema: { type: 'object', properties: { version: { type: 'string' } } },
            annotations: { destructiveHint: true },
          },
        ],
      }
    },
    async ping() {
      calls.push({ type: 'ping', server: server.name })
      return {}
    },
    async callTool(params, _schema, options) {
      options?.onprogress?.({ progress: 1, total: 2 })
      calls.push({ type: 'tool', params })
      return {
        content: [{ type: 'text', text: `Found documentation for ${params.arguments.query}` }],
        structuredContent: { matches: 1 },
      }
    },
    getServerVersion() {
      return { name: 'fake-mcp', version: '1.0.0' }
    },
    getServerCapabilities() {
      return { tools: { listChanged: true } }
    },
    async close() {
      calls.push({ type: 'close', server: server.name })
    },
    emitToolsChanged(tools) {
      return handlers.onToolsChanged(null, tools)
    },
  }
}

test('MCP service persists servers, discovers tools, and exposes Pi custom tools', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mcp-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const calls = []
  const service = new McpService({
    path: join(directory, 'pisper-mcp.json'),
    cwd: directory,
    createClient: (server, handlers) => createFakeClient(server, handlers, calls),
    createTransport: (server) => ({
      kind: server.transport,
      endpoint: server.url || server.command,
    }),
  })
  await service.init()

  const created = await service.add({
    name: 'Docs',
    transport: 'http',
    url: 'https://user:password@mcp.example.com/mcp?token=secret',
    headers: { Authorization: 'Bearer secret' },
  })
  assert.equal(created.services.length, 1)
  assert.equal(created.services[0].status, 'online')
  assert.equal(created.services[0].toolCount, 2)
  assert.doesNotMatch(created.services[0].endpoint, /password|secret/)
  assert.equal(created.services[0].auth, 'headers')
  assert.equal(created.services[0].authCount, 1)
  assert.equal(created.tools.find((tool) => tool.name === 'search_docs').risk, 'low')
  assert.equal(created.tools.find((tool) => tool.name === 'publish_release').risk, 'high')

  const definitions = await service.createToolDefinitions()
  assert.equal(definitions.length, 2)
  const search = definitions.find((tool) => tool.name.includes('search_docs'))
  const publish = definitions.find((tool) => tool.name.includes('publish_release'))
  assert.equal(service.getToolRisk(search.name), 'medium')
  assert.equal(service.getToolRisk(publish.name), 'high')
  const updates = []
  const result = await search.execute(
    'tool-1',
    { query: 'MCP' },
    new AbortController().signal,
    (update) => updates.push(update),
  )
  assert.match(result.content[0].text, /Found documentation for MCP/)
  assert.equal(result.details.structuredContent.matches, 1)
  assert.equal(updates[0].details.progress.progress, 1)
  assert.equal(service.dashboard().calls[0].toolName, 'search_docs')

  const serverId = created.services[0].id
  const disabled = await service.setToolEnabled(serverId, 'publish_release', false)
  assert.equal(disabled.tools.find((tool) => tool.name === 'publish_release').enabled, false)
  assert.equal((await service.createToolDefinitions()).length, 1)
  const serviceDisabled = await service.update(serverId, { enabled: false })
  assert.equal(serviceDisabled.services[0].status, 'disabled')
  assert.equal(serviceDisabled.metrics.availableTools, 0)
  assert.equal((await service.update(serverId, { enabled: true })).services[0].status, 'online')

  await service.dispose()
  const restored = new McpService({ path: join(directory, 'pisper-mcp.json'), cwd: directory })
  await restored.init()
  const snapshot = restored.dashboard()
  assert.equal(snapshot.services[0].name, 'Docs')
  assert.equal(
    snapshot.services[0].tools.find((tool) => tool.name === 'publish_release').enabled,
    false,
  )
  assert.equal(snapshot.calls[0].toolName, 'search_docs')
})

test('stdio MCP dashboard exposes an unambiguous executable and working directory', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mcp-stdio-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new McpService({
    path: join(directory, 'pisper-mcp.json'),
    cwd: directory,
    createClient: (server, handlers) => createFakeClient(server, handlers, []),
    createTransport: () => ({}),
  })
  await service.init()
  const executable = join(directory, 'mcp server.exe')
  await writeFile(executable, '', 'utf8')
  const dashboard = await service.add({
    name: 'Local MCP',
    transport: 'stdio',
    command: executable,
    args: ['--app', 'desktop'],
  })
  const server = dashboard.services[0]
  assert.equal(server.command, resolve(executable))
  assert.deepEqual(server.args, ['--app', 'desktop'])
  assert.equal(server.workingDirectory, resolve(directory))
  assert.equal(server.endpoint, `"${resolve(executable)}" --app desktop`)
})

test('stdio MCP validation rejects escaped control characters and missing executable paths', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mcp-validation-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new McpService({ path: join(directory, 'state.json'), cwd: directory })
  await service.init()

  await assert.rejects(
    service.add({
      name: 'Broken escapes',
      transport: 'stdio',
      command: `C:Userspencil\u000bisualmcp.exe`,
      enabled: false,
    }),
    /control characters/,
  )
  await assert.rejects(
    service.add({
      name: 'Missing executable',
      transport: 'stdio',
      command: join(directory, 'missing-mcp.exe'),
      enabled: false,
    }),
    /executable does not exist/,
  )
  assert.equal(service.dashboard().services.length, 0)
})

test('MCP service connects to a real Streamable HTTP endpoint with configured headers', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mcp-http-'))
  let service
  let httpServer
  t.after(async () => {
    await service?.dispose()
    if (httpServer?.listening) await new Promise((resolveClose) => httpServer.close(resolveClose))
    await rm(directory, { recursive: true, force: true })
  })
  const authorizationHeaders = []
  httpServer = createServer(async (request, response) => {
    authorizationHeaders.push(request.headers.authorization || '')
    if (request.method === 'GET' || request.method === 'DELETE') {
      response.writeHead(405)
      response.end()
      return
    }
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    const message = JSON.parse(Buffer.concat(chunks).toString('utf8'))
    if (message.id === undefined) {
      response.writeHead(202)
      response.end()
      return
    }
    let result = {}
    if (message.method === 'initialize') {
      result = {
        protocolVersion: message.params.protocolVersion,
        capabilities: { tools: {} },
        serverInfo: { name: 'http-fixture', version: '1.0.0' },
      }
    } else if (message.method === 'tools/list') {
      result = {
        tools: [
          {
            name: 'http_echo',
            description: 'Echo over Streamable HTTP',
            inputSchema: {
              type: 'object',
              properties: { text: { type: 'string' } },
              required: ['text'],
            },
            annotations: { readOnlyHint: true },
          },
        ],
      }
    } else if (message.method === 'tools/call') {
      result = { content: [{ type: 'text', text: `http:${message.params.arguments.text}` }] }
    }
    response.writeHead(200, { 'Content-Type': 'application/json' })
    response.end(JSON.stringify({ jsonrpc: '2.0', id: message.id, result }))
  })
  await new Promise((resolveListen) => httpServer.listen(0, '127.0.0.1', resolveListen))
  const port = httpServer.address().port

  service = new McpService({ path: join(directory, 'state.json'), cwd: directory })
  await service.init()
  const dashboard = await service.add({
    name: 'HTTP Fixture',
    transport: 'http',
    url: `http://127.0.0.1:${port}/mcp`,
    headers: { Authorization: 'Bearer fixture-secret' },
    requestTimeoutMs: 10_000,
  })
  assert.equal(dashboard.services[0].status, 'online')
  assert.equal(dashboard.tools[0].name, 'http_echo')
  const [echo] = await service.createToolDefinitions()
  const result = await echo.execute('http-call', { text: 'hello' }, new AbortController().signal)
  assert.equal(result.content[0].text, 'http:hello')
  assert.ok(authorizationHeaders.length >= 3)
  assert.ok(authorizationHeaders.every((value) => value === 'Bearer fixture-secret'))
})

test('MCP service remains compatible with legacy HTTP plus SSE servers', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mcp-sse-'))
  let service
  let httpServer
  let eventStream
  t.after(async () => {
    await service?.dispose()
    eventStream?.end()
    httpServer?.closeAllConnections?.()
    if (httpServer?.listening) await new Promise((resolveClose) => httpServer.close(resolveClose))
    await rm(directory, { recursive: true, force: true })
  })
  const authorizationHeaders = []
  httpServer = createServer(async (request, response) => {
    authorizationHeaders.push(request.headers.authorization || '')
    if (request.method === 'GET' && request.url === '/sse') {
      response.writeHead(200, {
        'Content-Type': 'text/event-stream',
        'Cache-Control': 'no-cache',
        Connection: 'keep-alive',
      })
      eventStream = response
      response.write(
        `event: endpoint\ndata: http://127.0.0.1:${httpServer.address().port}/messages\n\n`,
      )
      return
    }
    if (request.method === 'POST' && request.url === '/messages') {
      const chunks = []
      for await (const chunk of request) chunks.push(chunk)
      const message = JSON.parse(Buffer.concat(chunks).toString('utf8'))
      response.writeHead(202)
      response.end()
      if (message.id === undefined) return
      let result = {}
      if (message.method === 'initialize') {
        result = {
          protocolVersion: message.params.protocolVersion,
          capabilities: { tools: {} },
          serverInfo: { name: 'sse-fixture', version: '1.0.0' },
        }
      } else if (message.method === 'tools/list') {
        result = {
          tools: [
            {
              name: 'sse_echo',
              description: 'Echo over legacy SSE',
              inputSchema: {
                type: 'object',
                properties: { text: { type: 'string' } },
                required: ['text'],
              },
            },
          ],
        }
      } else if (message.method === 'tools/call') {
        result = { content: [{ type: 'text', text: `sse:${message.params.arguments.text}` }] }
      }
      eventStream.write(
        `event: message\ndata: ${JSON.stringify({ jsonrpc: '2.0', id: message.id, result })}\n\n`,
      )
      return
    }
    response.writeHead(404)
    response.end()
  })
  await new Promise((resolveListen) => httpServer.listen(0, '127.0.0.1', resolveListen))
  const port = httpServer.address().port

  service = new McpService({ path: join(directory, 'state.json'), cwd: directory })
  await service.init()
  const dashboard = await service.add({
    name: 'SSE Fixture',
    transport: 'sse',
    url: `http://127.0.0.1:${port}/sse`,
    headers: { Authorization: 'Bearer sse-secret' },
    requestTimeoutMs: 10_000,
  })
  assert.equal(dashboard.services[0].status, 'online')
  assert.equal(dashboard.tools[0].name, 'sse_echo')
  const [echo] = await service.createToolDefinitions()
  const result = await echo.execute('sse-call', { text: 'hello' }, new AbortController().signal)
  assert.equal(result.content[0].text, 'sse:hello')
  assert.ok(authorizationHeaders.length >= 4)
  assert.ok(authorizationHeaders.every((value) => value === 'Bearer sse-secret'))
})

test('MCP service connects to a real SDK stdio server', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-mcp-stdio-'))
  let service
  t.after(async () => {
    await service?.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  const sdkUrl = (path) =>
    pathToFileURL(resolve('node_modules/@modelcontextprotocol/sdk/dist/esm', path)).href
  const fixturePath = join(directory, 'fixture-server.mjs')
  await writeFile(
    fixturePath,
    `
import { Server } from ${JSON.stringify(sdkUrl('server/index.js'))}
import { StdioServerTransport } from ${JSON.stringify(sdkUrl('server/stdio.js'))}
import { CallToolRequestSchema, ListToolsRequestSchema } from ${JSON.stringify(sdkUrl('types.js'))}
const server = new Server({ name: 'fixture', version: '1.0.0' }, { capabilities: { tools: {} } })
server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: [{ name: 'echo', description: 'Echo text', inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] }, annotations: { readOnlyHint: true } }],
}))
server.setRequestHandler(CallToolRequestSchema, async (request) => ({ content: [{ type: 'text', text: 'echo:' + request.params.arguments.text }] }))
await server.connect(new StdioServerTransport())
`,
    'utf8',
  )

  service = new McpService({ path: join(directory, 'state.json'), cwd: directory })
  await service.init()
  const dashboard = await service.add({
    name: 'Fixture',
    transport: 'stdio',
    command: process.execPath,
    args: [fixturePath],
    cwd: directory,
    requestTimeoutMs: 10_000,
  })
  assert.equal(dashboard.services[0].status, 'online')
  assert.equal(dashboard.tools[0].name, 'echo')

  const [echo] = await service.createToolDefinitions()
  const result = await echo.execute('stdio-call', { text: 'hello' }, new AbortController().signal)
  assert.equal(result.content[0].text, 'echo:hello')
})

test('MCP connection specs support URLs, stdio commands, and JSON configuration', () => {
  const http = parseMcpServerInput('https://example.com/mcp')
  assert.equal(http.transport, 'http')
  assert.equal(http.url, 'https://example.com/mcp')

  const stdio = parseMcpServerInput(
    'npx -y @modelcontextprotocol/server-filesystem "C:\\Work Space"',
    'C:\\workspace',
  )
  assert.equal(stdio.transport, 'stdio')
  assert.equal(stdio.command, 'npx')
  assert.deepEqual(stdio.args, ['-y', '@modelcontextprotocol/server-filesystem', 'C:\\Work Space'])

  const json = parseMcpServerInput(
    JSON.stringify({
      name: 'Private Docs',
      transport: 'http',
      url: 'https://example.com/private-mcp',
      headers: { Authorization: 'Bearer secret' },
    }),
  )
  assert.equal(json.name, 'Private Docs')
  assert.equal(json.headers.Authorization, 'Bearer secret')

  const nested = parseMcpServerInput(
    JSON.stringify({
      mcpServers: {
        filesystem: {
          command: 'npx',
          args: ['-y', '@modelcontextprotocol/server-filesystem', '.'],
        },
      },
    }),
  )
  assert.equal(nested.name, 'filesystem')
  assert.equal(nested.transport, 'stdio')
})
