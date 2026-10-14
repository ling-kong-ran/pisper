import assert from 'node:assert/strict'
import test from 'node:test'
import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { McpService, parseMcpServerInput } from '../services/mcp-service.mjs'
import { mcpApi, parseMcpDashboard } from '../../src/features/mcp/mcp-api.ts'
import {
  MCP_QUERY_KEY,
  mcpDashboardQueryOptions,
  mcpMutationOptions,
} from '../../src/features/mcp/mcp-queries.ts'

const dashboard = (name = 'Example') => ({
  services: [{ id: 'server / 1', name, enabled: true, transport: 'stdio', status: 'offline' }],
  tools: [],
  calls: [],
})
const deferred = () => Promise.withResolvers()

test('MCP client accepts the runtime dashboard contract without connecting a process', () => {
  const service = new McpService({ createClient: () => assert.fail('must not connect') })
  const server = parseMcpServerInput({
    name: 'Example',
    transport: 'stdio',
    command: 'example',
    args: [],
  })
  server.tools = [{ name: 'read', description: 'Read an item', inputSchema: { type: 'object' } }]
  service.state.servers.push(server)
  service.calls.push({
    id: 'c1',
    serviceId: server.id,
    toolName: 'read',
    timestamp: '2026-09-22T00:00:00.000Z',
    status: 'ok',
    durationMs: 10,
  })
  const decoded = parseMcpDashboard(service.dashboard())
  assert.equal(decoded.services[0].name, 'Example')
  assert.equal(decoded.tools[0].name, 'read')
  assert.equal(decoded.calls[0].durationMs, 10)
})

test('MCP decoding handles empty and older dashboards while rejecting malformed fields', () => {
  assert.equal(parseMcpDashboard({ services: [] }).services.length, 0)
  assert.equal(parseMcpDashboard(dashboard()).services[0].name, 'Example')
  for (const value of [
    null,
    [],
    {},
    { services: 'bad' },
    { services: [null] },
    { services: [{ id: 's', name: 'n', enabled: 'true' }] },
    { services: [], tools: null },
    { services: [], metrics: { errorRate: 'bad' } },
  ]) {
    assert.throws(() => parseMcpDashboard(value), {
      kind: 'protocol',
      data: { code: 'INVALID_RESPONSE' },
    })
  }
  const input = dashboard()
  input.services[0].futureField = { ignored: true }
  assert.equal(parseMcpDashboard(input).services[0].name, 'Example')
})

test('MCP dashboard reads never force connections and mutation paths encode IDs', async (t) => {
  const requests = []
  t.mock.method(globalThis, 'fetch', async (url, options) => {
    requests.push({ url, options })
    return Response.json(options.method === 'DELETE' ? { deleted: true } : dashboard())
  })
  const controller = new AbortController()
  await mcpApi.dashboard(controller.signal)
  await mcpApi.setToolEnabled('server / 1', 'tool/?', false)
  await mcpApi.test('server / 1')
  await mcpApi.remove('server / 1')
  assert.equal(requests[0].url, '/api/mcp?refresh=0')
  assert.ok(requests[0].options.signal instanceof AbortSignal)
  assert.equal(requests[1].url, '/api/mcp/server%20%2F%201/tools/tool%2F%3F')
  assert.deepEqual(JSON.parse(requests[1].options.body), { enabled: false })
  assert.equal(requests[2].options.method, 'POST')
  assert.equal(requests[3].options.method, 'DELETE')
  assert.equal(requests[4].url, '/api/mcp?refresh=0')
})

test('MCP mutation cancels a stale read before publishing its new snapshot', async (t) => {
  const entered = deferred()
  const oldResponse = deferred()
  let signal
  let readFinished
  const dashboardRead = mcpApi.dashboard
  t.mock.method(mcpApi, 'dashboard', (...args) => {
    readFinished = dashboardRead(...args)
    return readFinished
  })
  t.mock.method(globalThis, 'fetch', async (_url, options) => {
    signal = options.signal
    entered.resolve()
    // 模拟已在网络中、忽略取消并晚到的旧响应。
    return oldResponse.promise
  })
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  try {
    const pending = client.fetchQuery(mcpDashboardQueryOptions()).catch((error) => error)
    await entered.promise
    const mutation = client.getMutationCache().build(client, mcpMutationOptions(client))
    const next = parseMcpDashboard(dashboard('Updated'))
    await mutation.execute(async () => {
      assert.equal(signal.aborted, true)
      return next
    })
    oldResponse.resolve(Response.json(dashboard('Stale')))
    await readFinished
    await pending
    assert.deepEqual(client.getQueryData(MCP_QUERY_KEY), next)
  } finally {
    oldResponse.resolve(Response.json(dashboard()))
    await client.cancelQueries()
    client.clear()
  }
})

test('last MCP observer leaving cancels its request; remount starts a fresh read', async (t) => {
  const entered = deferred()
  const aborted = deferred()
  let count = 0
  t.mock.method(globalThis, 'fetch', async (_url, { signal }) => {
    count += 1
    if (count > 1) return Response.json(dashboard('Remounted'))
    return new Promise((_resolve, reject) => {
      signal.addEventListener(
        'abort',
        () => {
          aborted.resolve()
          reject(signal.reason)
        },
        { once: true },
      )
      entered.resolve()
    })
  })
  const client = new QueryClient()
  const observer = new QueryObserver(client, mcpDashboardQueryOptions())
  const unsubscribe = observer.subscribe(() => {})
  try {
    await entered.promise
    unsubscribe()
    await aborted.promise
    const data = await client.fetchQuery(mcpDashboardQueryOptions())
    assert.equal(data.services[0].name, 'Remounted')
    assert.equal(count, 2)
  } finally {
    unsubscribe()
    await client.cancelQueries()
    client.clear()
  }
})

test('failed MCP mutations retain the last snapshot and are never retried automatically', async () => {
  const client = new QueryClient()
  const before = parseMcpDashboard(dashboard())
  client.setQueryData(MCP_QUERY_KEY, before)
  let attempts = 0
  try {
    const mutation = client.getMutationCache().build(client, mcpMutationOptions(client))
    await assert.rejects(
      mutation.execute(async () => {
        attempts += 1
        throw new Error('failed')
      }),
      /failed/,
    )
    assert.equal(attempts, 1)
    assert.deepEqual(client.getQueryData(MCP_QUERY_KEY), before)
    assert.equal(client.getQueryState(MCP_QUERY_KEY).isInvalidated, true)
  } finally {
    client.clear()
  }
})
