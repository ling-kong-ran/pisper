// 原生组件目录的共享查询（A7 S4 起取代旧 iframe 组件的 useCustomUiComponents）：所有放置宿主与组件页
// 读同一份缓存，只请求一次；只有最后一个使用者离开时才取消请求。旧 iframe 组件的视图凭证与桥测试随旧机制删除。
import assert from 'node:assert/strict'
import test from 'node:test'
import { QueryClient, QueryObserver } from '@tanstack/react-query'
import { componentCatalogQueryOptions } from '../../src/features/custom-ui/placements.ts'

const ROW = {
  id: 'fixture',
  kind: 'native',
  status: 'ready',
  name: 'Fixture',
  version: '1.0.0',
  description: '',
  directory: '~/.pisper/agent/custom-ui/fixture',
  enabled: true,
  source: 'local',
  rev: 'a'.repeat(24),
  entry: 'component.js',
  manifest: {
    id: 'fixture',
    runtime: 'native',
    name: 'Fixture',
    version: '1.0.0',
    description: '',
    entry: 'component.js',
    styles: [],
    icon: '',
    i18n: {},
    props: null,
    placements: [{ surface: 'feature-page' }],
    capabilities: {},
    sdk: 1,
    minHostVersion: '',
  },
  errors: [],
  updatedAt: '2026-10-31T00:00:00.000Z',
}
const CATALOG = {
  root: '~/.pisper/agent/custom-ui',
  features: { import: false, versions: false, export: false, trash: 'move' },
  components: [ROW],
}

test('component catalog shares one read and only aborts when its final consumer leaves', async (t) => {
  const entered = Promise.withResolvers()
  const aborted = Promise.withResolvers()
  let requests = 0
  let requestSignal
  t.mock.method(globalThis, 'fetch', async (_url, { signal }) => {
    requests += 1
    requestSignal = signal
    if (requests > 1) return Response.json(CATALOG)
    entered.resolve()
    return new Promise((_resolve, reject) =>
      signal.addEventListener(
        'abort',
        () => {
          aborted.resolve()
          reject(signal.reason)
        },
        { once: true },
      ),
    )
  })
  const client = new QueryClient()
  const a = new QueryObserver(client, componentCatalogQueryOptions()).subscribe(() => {})
  const b = new QueryObserver(client, componentCatalogQueryOptions()).subscribe(() => {})
  try {
    await entered.promise
    assert.equal(requests, 1)
    a()
    assert.equal(requestSignal.aborted, false)
    b()
    await aborted.promise
    const next = await client.fetchQuery(componentCatalogQueryOptions())
    assert.equal(next.components[0].id, 'fixture')
    assert.equal(next.components[0].kind, 'native')
    assert.equal(next.legacyShape, false)
    assert.equal(requests, 2)
  } finally {
    a()
    b()
    await client.cancelQueries()
    client.clear()
  }
})

test('catalog query is disabled until the Runtime declares components.native', async (t) => {
  let requests = 0
  t.mock.method(globalThis, 'fetch', async () => {
    requests += 1
    return Response.json(CATALOG)
  })
  const client = new QueryClient()
  const observer = new QueryObserver(client, componentCatalogQueryOptions(false))
  const stop = observer.subscribe(() => {})
  try {
    await new Promise((resolve) => setTimeout(resolve, 10))
    assert.equal(requests, 0)
    assert.equal(observer.getCurrentResult().fetchStatus, 'idle')
  } finally {
    stop()
    client.clear()
  }
})
