// 原生组件的 HTTP 接口：按 rev 服务 ESM 模块（鉴权、缓存头、拒绝规则、SVG CSP、无 ACAO），
// 以及 describe、模板创建、启用、删除（回收站）与实例路由，和 component_changed 的触发点。
import assert from 'node:assert/strict'
import { lstat, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { authorizeDesktopRequest } from '../desktop-sidecar-auth.mjs'
import { CustomUiService } from '../services/custom-ui-service.mjs'

async function writeComponent(root, id, manifest, files = {}) {
  const dir = join(root, id)
  await mkdir(dir, { recursive: true })
  await writeFile(
    join(dir, 'manifest.json'),
    typeof manifest === 'string' ? manifest : JSON.stringify(manifest),
  )
  for (const [path, content] of Object.entries(files)) {
    await mkdir(join(dir, path, '..'), { recursive: true })
    await writeFile(join(dir, path), content)
  }
  return dir
}

// 与旧 iframe 凭证测试同一套脚手架：真实服务器 + 桌面 Cookie 鉴权 + createApiHandler。
async function startServer(t, { capabilities } = {}) {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-module-'))
  const service = new CustomUiService({ dataDir: directory })
  const events = []
  await service.start({
    onChange: (id, detail) => events.push({ id, ...detail }),
    hostVersion: '0.6.5',
  })
  const handler = createApiHandler({ capabilities }, { customUi: service })
  const server = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    const url = new URL(req.url, origin)
    if (authorizeDesktopRequest(req, res, url, { token: 'fixture', origin })) return
    await handler(req, res, url)
  })
  t.after(async () => {
    service.dispose()
    await new Promise((resolveClose, reject) =>
      server.close((error) => (error ? reject(error) : resolveClose())),
    )
    await rm(directory, { recursive: true, force: true })
  })
  await new Promise((resolveListen) => server.listen(0, '127.0.0.1', resolveListen))
  const origin = `http://127.0.0.1:${server.address().port}`
  const headers = { Cookie: '__pisper_desktop=fixture', Origin: origin }
  const call = (method, path, body) =>
    fetch(origin + path, {
      method,
      headers: body === undefined ? headers : { ...headers, 'Content-Type': 'application/json' },
      ...(body === undefined
        ? {}
        : { body: typeof body === 'string' ? body : JSON.stringify(body) }),
    })
  const callJson = async (method, path, body) => {
    const response = await call(method, path, body)
    return { status: response.status, body: await response.json() }
  }
  return { directory, service, events, origin, call, callJson }
}

test('模块路由需要鉴权：没有桌面 Cookie 时 401', async (t) => {
  const { service, origin } = await startServer(t)
  await writeComponent(
    service.root,
    'demo',
    { runtime: 'native', name: 'Demo' },
    { 'component.js': 'export default 1' },
  )
  const { rev } = (await service.inspect('demo')).row
  for (const path of [
    `/api/custom-ui/components/demo/r/${rev}/component.js`,
    '/api/custom-ui/components/demo/describe',
    '/api/custom-ui/instances',
  ]) {
    assert.equal((await fetch(origin + path)).status, 401, path)
  }
})

test('按 rev 服务模块：当前 rev 永久缓存，旧 rev 不缓存但返回当前内容；没有沙箱 CSP 与 ACAO', async (t) => {
  const { service, call } = await startServer(t)
  await writeComponent(
    service.root,
    'demo',
    { runtime: 'native', name: 'Demo', styles: ['style.css'] },
    {
      'component.js': "import { label } from './lib/util.js'\nexport default () => label",
      'lib/util.js': "export const label = 'v1'",
      'style.css': '.demo { color: var(--color-content) }',
      'icon.svg': '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
      'data/deep/x.json': '{}',
    },
  )
  const { rev } = (await service.inspect('demo')).row
  assert.match(rev, /^[a-f0-9]{24}$/)
  const entry = await call('GET', `/api/custom-ui/components/demo/r/${rev}/component.js`)
  assert.equal(entry.status, 200)
  assert.equal(entry.headers.get('content-type'), 'text/javascript; charset=utf-8')
  assert.equal(entry.headers.get('cache-control'), 'private, max-age=31536000, immutable')
  assert.equal(entry.headers.get('x-content-type-options'), 'nosniff')
  assert.equal(entry.headers.get('cross-origin-resource-policy'), 'same-origin')
  assert.equal(entry.headers.get('access-control-allow-origin'), null)
  assert.equal(entry.headers.get('content-security-policy'), null)
  assert.match(await entry.text(), /from '\.\/lib\/util\.js'/)

  const nested = await call('GET', `/api/custom-ui/components/demo/r/${rev}/lib/util.js`)
  assert.equal(nested.status, 200)
  assert.equal(await nested.text(), "export const label = 'v1'")
  const deep = await call('GET', `/api/custom-ui/components/demo/r/${rev}/data/deep/x.json`)
  assert.equal(deep.status, 200)
  assert.match(deep.headers.get('content-type'), /application\/json/)
  await deep.text()
  const css = await call('GET', `/api/custom-ui/components/demo/r/${rev}/style.css`)
  assert.match(css.headers.get('content-type'), /text\/css/)
  await css.text()
  const svg = await call('GET', `/api/custom-ui/components/demo/r/${rev}/icon.svg`)
  assert.equal(svg.status, 200)
  assert.equal(
    svg.headers.get('content-security-policy'),
    "default-src 'none'; style-src 'unsafe-inline'; sandbox",
  )
  assert.equal(svg.headers.get('access-control-allow-origin'), null)
  await svg.text()

  // 改了文件：rev 变化；旧 rev 照样返回当前内容，但不能被缓存成旧版本。
  await writeFile(join(service.root, 'demo', 'lib', 'util.js'), "export const label = 'v2'")
  const next = (await service.inspect('demo')).row.rev
  assert.notEqual(next, rev)
  const stale = await call('GET', `/api/custom-ui/components/demo/r/${rev}/lib/util.js`)
  assert.equal(stale.status, 200)
  assert.equal(stale.headers.get('cache-control'), 'no-store')
  assert.equal(await stale.text(), "export const label = 'v2'")
  const fresh = await call('GET', `/api/custom-ui/components/demo/r/${next}/lib/util.js`)
  assert.equal(fresh.headers.get('cache-control'), 'private, max-age=31536000, immutable')
  await fresh.text()
})

test('模块路由拒绝 HTML、点段、manifest、符号链接、越界与未知类型；旧版与损坏组件 409', async (t) => {
  const { service, directory, callJson } = await startServer(t)
  const dir = await writeComponent(
    service.root,
    'demo',
    { runtime: 'native', name: 'Demo' },
    {
      'component.js': 'export default 1',
      'page.html': '<script>alert(1)</script>',
      'page.htm': '<p>',
      '.secret.js': 'export const token = 1',
      'notes.txt': 'plain',
    },
  )
  await mkdir(join(dir, '.hidden'))
  await writeFile(join(dir, '.hidden', 'x.js'), 'export default 1')
  await writeFile(join(directory, 'outside.js'), 'export const leaked = 1')
  await symlink(join(directory, 'outside.js'), join(dir, 'escape.js'))
  await symlink(join(dir, 'component.js'), join(dir, 'alias.js'))
  await writeComponent(
    service.root,
    'other',
    { runtime: 'native', name: 'Other' },
    { 'component.js': 'export default 2' },
  )
  const { rev } = (await service.inspect('demo')).row
  const base = `/api/custom-ui/components/demo/r/${rev}`
  for (const path of [
    'page.html',
    'page.htm',
    '.secret.js',
    '.hidden/x.js',
    'manifest.json',
    'escape.js',
    'alias.js',
    'notes.txt',
    'missing.js',
    '..%2Fother%2Fcomponent.js',
    '%2E%2E/other/component.js',
  ]) {
    const response = await callJson('GET', `${base}/${path}`)
    assert.equal(response.status, 404, path)
    assert.equal(response.body.code, 'component_asset_missing', path)
  }
  assert.equal(
    (await callJson('GET', `/api/custom-ui/components/demo/r/not-a-rev/component.js`)).status,
    404,
  )
  assert.equal(
    (await callJson('GET', `/api/custom-ui/components/missing/r/${rev}/component.js`)).body.code,
    'component_not_found',
  )

  await writeComponent(
    service.root,
    'old',
    { name: 'Old', entry: 'index.html' },
    { 'index.js': 'x' },
  )
  await writeComponent(service.root, 'broken', '{nope', { 'component.js': 'x' })
  await writeComponent(service.root, 'invalid', {
    runtime: 'native',
    name: 'Bad',
    entry: 'missing.js',
  })
  const legacy = await callJson('GET', `/api/custom-ui/components/old/r/${rev}/index.js`)
  assert.deepEqual([legacy.status, legacy.body.code], [409, 'component_legacy_unsupported'])
  assert.equal(
    (await callJson('GET', `/api/custom-ui/components/broken/r/${rev}/component.js`)).body.code,
    'component_invalid',
  )
  assert.equal(
    (await callJson('GET', `/api/custom-ui/components/invalid/r/${rev}/x.js`)).body.code,
    'component_invalid',
  )
})

test('停用的组件照常服务模块（工作室需要源码与预览）', async (t) => {
  const { service, call } = await startServer(t)
  await writeComponent(
    service.root,
    'demo',
    { runtime: 'native', name: 'Demo' },
    { 'component.js': 'export default 1' },
  )
  await service.setEnabled('demo', { enabled: false })
  const { rev, enabled } = (await service.inspect('demo')).row
  assert.equal(enabled, false)
  const response = await call('GET', `/api/custom-ui/components/demo/r/${rev}/component.js`)
  assert.equal(response.status, 200)
  await response.text()
})

test('components.native 为 false 的 Runtime 对新接口返回 409 能力缺失，组件列表不受影响', async (t) => {
  const { callJson } = await startServer(t, {
    capabilities: { features: { 'components.native': false } },
  })
  for (const path of ['/api/custom-ui/instances', '/api/custom-ui/components/demo/describe']) {
    const response = await callJson('GET', path)
    assert.equal(response.status, 409, path)
    assert.equal(response.body.capability, 'components.native')
  }
  assert.equal((await callJson('GET', '/api/custom-ui/components')).status, 200)
})

test('describe 返回行、文件清单、错误与 unsupported 扫描；损坏目录可见', async (t) => {
  const { service, callJson } = await startServer(t)
  await writeComponent(
    service.root,
    'bad',
    { runtime: 'native', name: 'Bad', props: { properties: { x: { type: 'object' } } } },
    { 'component.js': 'x' },
  )
  await mkdir(join(service.root, 'empty'))
  await writeComponent(
    service.root,
    'old',
    { name: 'Old', version: '2', description: 'iframe' },
    { 'index.html': '<p>' },
  )
  const bad = await callJson('GET', '/api/custom-ui/components/bad/describe')
  assert.equal(bad.status, 200)
  assert.equal(bad.body.component.kind, 'native')
  assert.equal(bad.body.component.status, 'invalid')
  assert.equal(bad.body.component.rev, undefined)
  assert.deepEqual(bad.body.errors, [
    { path: 'props.properties.x.type', code: 'props_type_unsupported' },
  ])
  assert.deepEqual(bad.body.scan, { status: 'unsupported' })
  assert.deepEqual(
    bad.body.files.map((file) => [file.path, file.type]),
    [
      ['component.js', 'text/javascript; charset=utf-8'],
      ['manifest.json', 'application/json; charset=utf-8'],
    ],
  )
  const empty = await callJson('GET', '/api/custom-ui/components/empty/describe')
  assert.deepEqual(
    [empty.body.component.kind, empty.body.component.status, empty.body.errors[0].code],
    ['broken', 'invalid', 'manifest_missing'],
  )
  const old = await callJson('GET', '/api/custom-ui/components/old/describe')
  assert.deepEqual(
    [
      old.body.component.kind,
      old.body.component.status,
      old.body.component.name,
      old.body.component.enabled,
    ],
    ['legacy', 'unsupported', 'Old', false],
  )
  assert.equal((await callJson('GET', '/api/custom-ui/components/missing/describe')).status, 404)

  const catalog = await service.catalog()
  assert.deepEqual(catalog.features, {
    import: false,
    versions: false,
    export: false,
    trash: 'move',
  })
  assert.deepEqual(
    catalog.components.map((row) => [row.id, row.kind, row.status]),
    [
      ['bad', 'native', 'invalid'],
      ['empty', 'broken', 'invalid'],
      ['old', 'legacy', 'unsupported'],
    ],
  )
  for (const row of catalog.components) {
    assert.equal(row.entryUrl, undefined)
    assert.equal(row.permissions, undefined)
    assert.equal(row.builtIn, undefined)
  }
})

test('GET /api/custom-ui/components 返回 v2 目录（A7 S4 切换）：旧组件是 legacy 行，没有旧字段与内置组件', async (t) => {
  const { service, callJson } = await startServer(t)
  await writeComponent(
    service.root,
    'old',
    { name: 'Old', permissions: ['notify'] },
    { 'index.html': '<p>' },
  )
  const listed = await callJson('GET', '/api/custom-ui/components')
  assert.equal(listed.status, 200)
  assert.deepEqual(listed.body.features, {
    import: false,
    versions: false,
    export: false,
    trash: 'move',
  })
  const old = listed.body.components.find((component) => component.id === 'old')
  assert.equal(old.kind, 'legacy')
  assert.equal(old.status, 'unsupported')
  assert.equal(old.permissions, undefined)
  assert.equal(old.entryUrl, undefined)
  assert.equal(
    listed.body.components.some(
      (component) => component.builtIn || component.id === 'pisper-island',
    ),
    false,
  )
})

test('模板创建：201、没有默认放置、id 自动去重；保留 id、非法 id、已存在与超大请求被拒绝', async (t) => {
  const { service, events, callJson, call } = await startServer(t)
  const notes = await callJson('POST', '/api/custom-ui/components', { template: 'notes' })
  assert.equal(notes.status, 201)
  assert.equal(notes.body.component.id, 'notes')
  assert.equal(notes.body.component.source, 'template')
  assert.equal(notes.body.component.enabled, true)
  assert.equal(notes.body.component.status, 'ready')
  assert.equal(notes.body.component.manifest.capabilities.storage.quotaKb, 64)
  assert.equal(
    (await callJson('POST', '/api/custom-ui/components', { template: 'notes' })).body.component.id,
    'notes-2',
  )
  const blank = await callJson('POST', '/api/custom-ui/components', {
    template: 'blank',
    name: 'CI Status',
  })
  assert.equal(blank.body.component.id, 'ci-status')
  assert.equal(blank.body.component.name, 'CI Status')
  const chinese = await callJson('POST', '/api/custom-ui/components', {
    template: 'blank',
    name: '构建状态',
  })
  assert.equal(chinese.body.component.id, 'component')
  assert.deepEqual(await service.instances.list(), { revision: 0, instances: [] })
  assert.deepEqual(
    events.map((event) => [event.id, event.reason, /^[a-f0-9]{24}$/.test(event.rev)]),
    [
      ['notes', 'created', true],
      ['notes-2', 'created', true],
      ['ci-status', 'created', true],
      ['component', 'created', true],
    ],
  )
  for (const [body, status, code] of [
    [{ template: 'notes', id: 'pisper-island' }, 409, 'component_id_reserved'],
    [{ template: 'notes', id: 'Bad Id' }, 400, 'component_id_invalid'],
    [{ template: 'notes', id: 'notes' }, 409, 'component_already_installed'],
    [{ template: 'weather' }, 400, 'component_template_invalid'],
    [{ template: 'blank', name: 'x'.repeat(121) }, 400, 'component_name_invalid'],
    [{ template: 'blank', extra: 1 }, 400, 'component_request_invalid'],
    [[1], 400, 'component_request_invalid'],
  ]) {
    const response = await callJson('POST', '/api/custom-ui/components', body)
    assert.deepEqual(
      [response.status, response.body.code, response.body.error],
      [status, code, code],
      JSON.stringify(body),
    )
  }
  const invalidJson = await callJson('POST', '/api/custom-ui/components', '{nope')
  assert.equal(invalidJson.body.code, 'component_request_invalid')
  const tooLarge = await call(
    'POST',
    '/api/custom-ui/components',
    JSON.stringify({ template: 'blank', name: 'x'.repeat(9000) }),
  )
  assert.equal(tooLarge.status, 413)
  assert.equal((await tooLarge.json()).code, 'component_request_too_large')
})

test('PATCH 启用与 DELETE：删除移入回收站，同时移走存储与实例，发出 deleted', async (t) => {
  const { service, directory, events, callJson } = await startServer(t)
  await callJson('POST', '/api/custom-ui/components', { template: 'notes' })
  const disabled = await callJson('PATCH', '/api/custom-ui/components/notes', { enabled: false })
  assert.equal(disabled.status, 200)
  assert.equal(disabled.body.component.enabled, false)
  assert.equal(events.at(-1).reason, 'state')
  await callJson('PATCH', '/api/custom-ui/components/notes', { enabled: true })
  const card = await callJson('POST', '/api/custom-ui/instances', {
    componentId: 'notes',
    surface: 'overview-card',
  })
  assert.equal(card.status, 201)
  await callJson('POST', '/api/custom-ui/instances', { componentId: 'notes', surface: 'floating' })
  await mkdir(join(directory, 'custom-ui-data'), { recursive: true })
  await writeFile(join(directory, 'custom-ui-data', 'notes.json'), '{"version":1,"values":{}}')

  const removed = await callJson('DELETE', '/api/custom-ui/components/notes')
  assert.equal(removed.status, 200)
  assert.equal(removed.body.ok, true)
  assert.match(removed.body.trashedAs, /^notes-\d{8}T\d{6}Z$/)
  assert.equal(await lstat(join(service.root, 'notes')).catch(() => null), null)
  const trash = join(directory, 'custom-ui-trash', removed.body.trashedAs)
  assert.deepEqual((await readdir(trash)).sort(), ['component', 'data.json', 'meta.json'])
  assert.ok(await lstat(join(trash, 'component', 'component.js')))
  const meta = JSON.parse(await readFile(join(trash, 'meta.json'), 'utf8'))
  assert.equal(meta.id, 'notes')
  assert.equal(meta.state.source, 'template')
  assert.equal(meta.instances.length, 2)
  assert.deepEqual((await service.instances.list()).instances, [])
  assert.equal(await service.state.get('notes'), null)
  assert.deepEqual(events.at(-1), { id: 'notes', rev: null, reason: 'deleted' })
  assert.equal((await callJson('DELETE', '/api/custom-ui/components/notes')).status, 404)

  // 旧版与损坏目录同样可以删除（规格 5.10：只提供删除，先进回收站）。
  await writeComponent(service.root, 'old', { name: 'Old' }, { 'index.html': '<p>' })
  await mkdir(join(service.root, 'empty'))
  assert.equal((await callJson('DELETE', '/api/custom-ui/components/old')).status, 200)
  assert.equal((await callJson('DELETE', '/api/custom-ui/components/empty')).status, 200)
  assert.equal(
    (await callJson('PATCH', '/api/custom-ui/components/missing', { enabled: true })).body.code,
    'component_not_found',
  )
})

test('实例路由：GET 按放置过滤，POST / PATCH / DELETE / PUT 走结构化错误', async (t) => {
  const { events, callJson } = await startServer(t)
  await callJson('POST', '/api/custom-ui/components', { template: 'usage-today' })
  const created = await callJson('POST', '/api/custom-ui/instances', {
    componentId: 'usage-today',
    surface: 'overview-card',
    size: 'S',
    title: '今天',
  })
  assert.equal(created.status, 201)
  assert.equal(created.body.revision, 1)
  const instanceId = created.body.instance.instanceId
  assert.match(instanceId, /^[0-9a-f-]{36}$/)
  await callJson('POST', '/api/custom-ui/instances', {
    componentId: 'usage-today',
    surface: 'feature-page',
  })
  const filtered = await callJson('GET', '/api/custom-ui/instances?surface=overview-card')
  assert.deepEqual(
    filtered.body.instances.map((instance) => instance.instanceId),
    [instanceId],
  )
  assert.equal((await callJson('GET', '/api/custom-ui/instances')).body.instances.length, 2)
  assert.equal(
    (await callJson('GET', '/api/custom-ui/instances?surface=board-card')).body.code,
    'component_surface_invalid',
  )
  const duplicate = await callJson('POST', '/api/custom-ui/instances', {
    componentId: 'usage-today',
    surface: 'feature-page',
  })
  assert.deepEqual([duplicate.status, duplicate.body.code], [409, 'component_instance_exists'])
  const unsupported = await callJson('POST', '/api/custom-ui/instances', {
    componentId: 'usage-today',
    surface: 'session-panel',
  })
  assert.deepEqual(
    [unsupported.status, unsupported.body.code],
    [400, 'component_surface_unsupported'],
  )
  const props = await callJson('POST', '/api/custom-ui/instances', {
    componentId: 'usage-today',
    surface: 'floating',
    props: { nope: 1 },
  })
  assert.deepEqual([props.status, props.body.code], [400, 'component_props_invalid'])
  assert.deepEqual(props.body.errors, [{ path: 'props.nope', code: 'props_unknown' }])

  const patched = await callJson('PATCH', `/api/custom-ui/instances/${instanceId}`, { size: 'M' })
  assert.equal(patched.body.instance.size, 'M')
  assert.equal(
    (await callJson('PATCH', '/api/custom-ui/instances/missing', { enabled: false })).status,
    404,
  )
  const { revision } = (await callJson('GET', '/api/custom-ui/instances')).body
  const conflict = await callJson('PUT', '/api/custom-ui/instances', {
    baseRevision: revision - 1,
    surface: 'overview-card',
    order: [instanceId],
  })
  assert.deepEqual(
    [conflict.status, conflict.body.code, conflict.body.revision],
    [409, 'component_instances_conflict', revision],
  )
  const reordered = await callJson('PUT', '/api/custom-ui/instances', {
    baseRevision: revision,
    surface: 'overview-card',
    order: [instanceId],
  })
  assert.equal(reordered.body.revision, revision + 1)
  const deleted = await callJson('DELETE', `/api/custom-ui/instances/${instanceId}`)
  assert.deepEqual(deleted.body, { revision: revision + 2 })
  assert.ok(events.filter((event) => event.reason === 'instances').length >= 5)
})
