import assert from 'node:assert/strict'
import test from 'node:test'
import { QueryClient } from '@tanstack/react-query'

// 网络是唯一替身；运行真实领域 parser、HTTP 取消和 Query 缓存。
import * as apiModule from '../../src/features/assets/assets-api.ts'
import * as queries from '../../src/features/assets/assets-queries.ts'
const asset = { id: 'asset/a', name: 'notes', kind: 'file', mimeType: 'text/plain', size: 4 }
function requireApi() {
  assert.equal(typeof apiModule.assetsApi?.list, 'function', 'asset API boundary is required')
  return apiModule.assetsApi
}
const originalFetch = globalThis.fetch
function http(t, handler) {
  globalThis.fetch = handler
  t.after(() => {
    globalThis.fetch = originalFetch
  })
}
const json = (value) =>
  new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } })
function gate() {
  let resolve
  const promise = new Promise((done) => {
    resolve = done
  })
  return { promise, resolve }
}

test('list combines normalized search, type and session without using an arbitrary asset URL', async (t) => {
  const api = requireApi()
  let path
  http(t, async (url) => {
    path = url
    return json({ assets: [asset] })
  })
  assert.equal(
    (await api.list({ query: '  notes  ', kind: 'file', sessionId: 'session/a' }))[0].id,
    'asset/a',
  )
  const url = new URL(path, 'http://fixture')
  assert.equal(url.pathname, '/api/assets')
  assert.equal(url.searchParams.get('query'), 'notes')
  assert.equal(url.searchParams.get('kind'), 'file')
  assert.equal(url.searchParams.get('sessionId'), 'session/a')
})

test('video filter uses real file registration and MIME type', async (t) => {
  const api = requireApi()
  http(t, async (url) => {
    assert.equal(new URL(url, 'http://fixture').searchParams.get('kind'), 'file')
    return json({ assets: [asset, { ...asset, id: 'video', mimeType: 'video/mp4' }] })
  })
  assert.deepEqual(
    (await api.list({ kind: 'video' })).map((a) => a.id),
    ['video'],
  )
})

test('invalid list fields and unsafe or missing link URLs fail as protocol errors', async (t) => {
  const api = requireApi()
  for (const payload of [
    { assets: {} },
    { assets: [{ ...asset, size: -1 }] },
    { assets: [{ ...asset, kind: 'link', url: 'javascript:alert(1)' }] },
    { assets: [{ ...asset, kind: 'link' }] },
  ]) {
    http(t, async () => json(payload))
    await assert.rejects(api.list({}), (error) => error.kind === 'protocol')
  }
})

test('preview is bounded, encoded by asset identity, and retains binary fallback', async (t) => {
  const api = requireApi()
  http(t, async (url) => {
    assert.equal(url, '/api/assets/asset%2Fa/content?preview=1')
    return json(asset)
  })
  assert.equal((await api.preview('asset/a')).kind, 'file')
  assert.equal(apiModule.assetMediaUrl('asset/a'), '/api/assets/asset%2Fa/download?inline=1')
})

test('attachment content preserves complete text and binary data while rejecting malformed content', async (t) => {
  const api = requireApi()
  let payload = { ...asset, kind: 'text', text: 'complete text' }
  http(t, async (url) => {
    assert.equal(url, '/api/assets/asset%2Fa/content')
    return json(payload)
  })
  assert.equal((await api.content('asset/a')).text, 'complete text')
  payload = { ...asset, kind: 'image', data: 'YWJj' }
  assert.equal((await api.content('asset/a')).data, 'YWJj')
  payload = { ...asset, kind: 'text' }
  await assert.rejects(api.content('asset/a'), (error) => error.kind === 'protocol')
})

test('read owner cancellation reaches the HTTP request and reports cancellation', async (t) => {
  const api = requireApi()
  const started = gate()
  http(
    t,
    (_url, { signal }) =>
      new Promise((_resolve, reject) => {
        started.resolve(signal)
        signal.addEventListener('abort', () => reject(signal.reason), { once: true })
      }),
  )
  const controller = new AbortController()
  const reading = api.preview('a', controller.signal)
  const signal = await started.promise
  controller.abort()
  await assert.rejects(reading, (error) => error.kind === 'cancelled')
  assert.equal(signal.aborted, true)
})

test('link creation validates protocols before POST and delete validates its acknowledgement', async (t) => {
  const api = requireApi()
  let calls = 0
  http(t, async (_url, options) => {
    calls++
    assert.equal(options.method, 'POST')
    return json({ ...asset, kind: 'link', url: 'https://example.com/' })
  })
  await assert.rejects(
    api.createLink({ name: 'bad', url: 'file:///tmp/a' }),
    (error) => error.data?.code === 'INVALID_ASSET_URL',
  )
  assert.equal(calls, 0)
  assert.equal(
    (await api.createLink({ name: 'site', url: 'https://example.com' })).url,
    'https://example.com/',
  )
  http(t, async () => json({ deleted: false }))
  await assert.rejects(api.remove('a'), (error) => error.kind === 'protocol')
})

test('scope and content generation separate cache identities; same-key refresh failure retains data', async (t) => {
  requireApi()
  assert.equal(typeof queries.assetsListQueryOptions, 'function')
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  t.after(() => client.clear())
  const a = queries.assetsListQueryOptions({ query: '', kind: 'image', sessionId: 'a' })
  const b = queries.assetsListQueryOptions({ query: '', kind: 'image', sessionId: 'b' })
  assert.notDeepEqual(a.queryKey, b.queryKey)
  assert.notDeepEqual(
    queries.assetPreviewQueryOptions('a', 1).queryKey,
    queries.assetPreviewQueryOptions('a', 2).queryKey,
  )
  http(t, async () => json({ assets: [asset] }))
  await client.fetchQuery(a)
  http(t, async () => new Response('unavailable', { status: 503 }))
  await assert.rejects(client.fetchQuery(a))
  assert.equal(client.getQueryData(a.queryKey)[0].id, 'asset/a')
  assert.equal(client.getQueryData(b.queryKey), undefined)
})

test('cancelled obsolete list cannot populate the next scope', async (t) => {
  requireApi()
  const client = new QueryClient()
  t.after(() => client.clear())
  const started = gate()
  const old = queries.assetsListQueryOptions({ sessionId: 'a' })
  const next = queries.assetsListQueryOptions({ sessionId: 'b' })
  http(t, (url, { signal }) => {
    if (new URL(url, 'http://fixture').searchParams.get('sessionId') === 'b')
      return Promise.resolve(json({ assets: [{ ...asset, id: 'b' }] }))
    started.resolve(signal)
    return new Promise((_resolve, reject) =>
      signal.addEventListener('abort', () => reject(signal.reason), { once: true }),
    )
  })
  const stale = client.fetchQuery(old).catch(() => undefined)
  const signal = await started.promise
  await client.cancelQueries({ queryKey: old.queryKey })
  await client.fetchQuery(next)
  await stale
  assert.equal(signal.aborted, true)
  assert.equal(client.getQueryData(next.queryKey)[0].id, 'b')
  assert.equal(client.getQueryData(old.queryKey), undefined)
})
