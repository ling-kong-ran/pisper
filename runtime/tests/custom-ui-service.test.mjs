// 自定义 UI 组件服务测试：清单校验、目录扫描、资产路径安全与桥接脚本。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { displayCustomUiPath } from '../services/custom-ui-path.mjs'
import {
  CUSTOM_UI_PERMISSIONS,
  CustomUiService,
  normalizeComponentManifest,
} from '../services/custom-ui-service.mjs'

async function createComponent(root, id, manifest, files = {}) {
  const dir = join(root, id)
  await mkdir(dir, { recursive: true })
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(manifest))
  for (const [path, content] of Object.entries(files)) {
    const target = join(dir, path)
    await mkdir(join(target, '..'), { recursive: true })
    await writeFile(target, content)
  }
  return dir
}

test('Windows 组件目录用可展开的主目录变量且不展示用户名', () => {
  const options = { home: 'C:\\Users\\Fixture User', platform: 'win32' }
  assert.equal(
    displayCustomUiPath('C:\\Users\\Fixture User\\.pisper\\agent\\custom-ui', options),
    '%USERPROFILE%\\.pisper\\agent\\custom-ui',
  )
  assert.equal(
    displayCustomUiPath('c:/users/FIXTURE USER/.pisper/agent/custom-ui/my-board', options),
    '%USERPROFILE%\\.pisper\\agent\\custom-ui\\my-board',
  )
  assert.equal(displayCustomUiPath('C:\\Users\\Fixture User\\', options), '%USERPROFILE%')
})

test('Windows 主目录边界不缩写相似用户名、自定义盘符和外部 UNC 目录', () => {
  const options = { home: 'C:\\Users\\Fixture', platform: 'win32' }
  for (const path of [
    'C:\\Users\\Fixture-other\\agent\\custom-ui',
    'C:\\Users\\Fixture\\..\\Other\\custom-ui',
    'D:\\Pisper Data\\agent\\custom-ui',
    '\\\\fileserver\\Pisper Data\\agent\\custom-ui',
  ]) {
    assert.equal(displayCustomUiPath(path, options), path)
  }
  assert.equal(
    displayCustomUiPath('\\\\fileserver\\profiles\\Fixture\\agent\\custom-ui', {
      home: '\\\\FILESERVER\\profiles\\Fixture',
      platform: 'win32',
    }),
    '%USERPROFILE%\\agent\\custom-ui',
  )
})

test('POSIX 组件目录保留主目录缩写并尊重大小写和配置覆盖', () => {
  const options = { home: '/home/fixture', platform: 'linux' }
  assert.equal(
    displayCustomUiPath('/home/fixture/.pisper/agent/custom-ui', options),
    '~/.pisper/agent/custom-ui',
  )
  assert.equal(displayCustomUiPath('/home/fixture/', options), '~')
  for (const path of [
    '/home/fixture-other/agent/custom-ui',
    '/home/Fixture/agent/custom-ui',
    '/srv/pisper data/agent/custom-ui',
  ]) {
    assert.equal(displayCustomUiPath(path, options), path)
  }
  assert.equal(
    displayCustomUiPath('/Users/fixture/.pisper/agent/custom-ui', {
      home: '/Users/fixture',
      platform: 'darwin',
    }),
    '~/.pisper/agent/custom-ui',
  )
})

test('主目录未知时不猜测组件路径归属', () => {
  assert.equal(
    displayCustomUiPath('C:\\Pisper\\agent\\custom-ui', { home: '', platform: 'win32' }),
    'C:\\Pisper\\agent\\custom-ui',
  )
  assert.equal(
    displayCustomUiPath('agent/custom-ui', { home: '/home/fixture', platform: 'linux' }),
    'agent/custom-ui',
  )
})

test('manifest normalization trims fields and filters unknown permissions', () => {
  const manifest = normalizeComponentManifest('demo', {
    name: '  Demo  ',
    version: '1.0.0',
    description: 'demo component',
    permissions: ['config.read', 'notify', 'rm -rf', 'config.read', 42],
  })
  assert.equal(manifest.name, 'Demo')
  assert.deepEqual(manifest.permissions, ['config.read', 'notify'])
  assert.equal(manifest.entry, 'index.html')
})

test('manifest normalization rejects invalid name and entry traversal', () => {
  assert.throws(() => normalizeComponentManifest('demo', { name: '' }), /name/)
  assert.throws(
    () => normalizeComponentManifest('demo', { name: 'x', entry: '../secret' }),
    /entry/,
  )
  assert.throws(
    () => normalizeComponentManifest('demo', { name: 'x', entry: '/etc/passwd' }),
    /entry/,
  )
  assert.throws(() => normalizeComponentManifest('demo', null), /JSON 对象/)
})

test('listComponents scans valid components and skips invalid directories', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-'))
  t.after(async () => rm(directory, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir: directory, builtinComponents: [] })
  // 目录不存在 → 空列表而不是抛错。
  assert.deepEqual(await service.listComponents(), {
    root: displayCustomUiPath(service.root),
    components: [],
  })
  const root = join(directory, 'custom-ui')
  await createComponent(
    root,
    'demo-board',
    { name: 'Demo Board', permissions: ['sessions.read'] },
    { 'index.html': '<html></html>' },
  )
  // 无 manifest 的目录、非法 id 目录、坏 manifest 都被跳过。
  await mkdir(join(root, 'no-manifest'), { recursive: true })
  await mkdir(join(root, 'UPPER CASE!'), { recursive: true })
  await createComponent(root, 'broken', { entry: 'index.html' })
  const listed = await service.listComponents()
  assert.equal(listed.root, displayCustomUiPath(root))
  assert.equal(listed.components.length, 1)
  const [component] = listed.components
  assert.equal(component.id, 'demo-board')
  assert.equal(component.name, 'Demo Board')
  assert.equal(component.entryUrl, '/api/custom-ui/components/demo-board/assets/index.html')
  assert.equal(component.directory, displayCustomUiPath(service.componentDir('demo-board')))
})

test('resolveAssetPath blocks traversal, hidden files, manifest and symlinks', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-assets-'))
  t.after(async () => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await createComponent(
    root,
    'demo',
    { name: 'Demo' },
    { 'index.html': '<html></html>', 'js/app.js': 'console.log(1)' },
  )
  const service = new CustomUiService({ dataDir: directory })
  const entry = await service.resolveAssetPath('demo', 'index.html')
  assert.ok(entry?.file.endsWith('index.html'))
  const nested = await service.resolveAssetPath('demo', 'js/app.js')
  assert.ok(nested?.file.endsWith('app.js'))
  // 路径穿越与绝对路径。
  assert.equal(await service.resolveAssetPath('demo', '../demo/index.html'), null)
  assert.equal(await service.resolveAssetPath('demo', '../../etc/passwd'), null)
  assert.equal(await service.resolveAssetPath('demo', '/etc/passwd'), null)
  assert.equal(await service.resolveAssetPath('demo', '..'), null)
  // manifest.json 与隐藏文件不作为资产。
  assert.equal(await service.resolveAssetPath('demo', 'manifest.json'), null)
  await writeFile(join(root, 'demo', '.secret'), 'x')
  assert.equal(await service.resolveAssetPath('demo', '.secret'), null)
  // 目录外符号链接被拒绝。
  const outside = join(directory, 'outside.txt')
  await writeFile(outside, 'secret')
  await symlink(outside, join(root, 'demo', 'linked.txt'))
  assert.equal(await service.resolveAssetPath('demo', 'linked.txt'), null)
  // 非法组件 id。
  assert.equal(await service.resolveAssetPath('../x', 'index.html'), null)
  // 不存在的文件。
  assert.equal(await service.resolveAssetPath('demo', 'missing.js'), null)
})

test('bridge script exposes the pisper global and permission names stay in sync', async () => {
  const service = new CustomUiService({ dataDir: '/nonexistent' })
  const script = service.bridgeScript()
  assert.match(script, /window\.pisper/)
  assert.match(script, /pisperBridge: 1/)
  assert.match(script, /getConfig/)
  assert.match(script, /listSessions/)
  assert.match(script, /notify/)
  assert.deepEqual([...CUSTOM_UI_PERMISSIONS].sort(), ['config.read', 'notify', 'sessions.read'])
})

test('serveAsset streams content with nosniff headers and 404 for unknown assets', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-serve-'))
  t.after(async () => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await createComponent(root, 'demo', { name: 'Demo' }, { 'index.html': '<h1>demo</h1>' })
  const service = new CustomUiService({ dataDir: directory })

  const served = await new Promise((resolvePromise) => {
    const chunks = []
    const res = {
      headers: null,
      writeHead(status, headers) {
        this.headers = { status, ...headers }
      },
      write(chunk) {
        chunks.push(chunk)
      },
      end(chunk) {
        if (chunk) chunks.push(chunk)
        resolvePromise({ headers: this.headers, body: Buffer.concat(chunks).toString('utf8') })
      },
      on() {},
      once() {},
      emit() {},
      destroy() {},
      get headersSent() {
        return Boolean(this.headers)
      },
    }
    void service.serveAsset({
      id: 'demo',
      path: 'index.html',
      res,
      json: (status, value) => resolvePromise({ headers: { status }, body: value }),
    })
  })
  assert.equal(served.headers.status, 200)
  assert.equal(served.headers['Content-Type'], 'text/html; charset=utf-8')
  assert.equal(served.headers['X-Content-Type-Options'], 'nosniff')
  assert.equal(served.headers['Cache-Control'], 'no-store')
  assert.match(served.headers['Content-Security-Policy'], /sandbox allow-scripts/)
  assert.doesNotMatch(served.headers['Content-Security-Policy'], /allow-same-origin/)
  assert.equal(served.headers['Referrer-Policy'], 'no-referrer')
  assert.match(served.body, /<h1>demo<\/h1>/)

  const missing = await new Promise((resolvePromise) => {
    void service.serveAsset({
      id: 'demo',
      path: '../manifest.json',
      res: {},
      json: (status, value) => resolvePromise({ status, value }),
    })
  })
  assert.equal(missing.status, 404)
})

test('custom UI HTTP API lists components, serves assets and bridge script', async (t) => {
  const { createApiHandler } = await import('../http/api-handler.mjs')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-api-'))
  t.after(async () => rm(directory, { recursive: true, force: true }))
  const root = join(directory, 'custom-ui')
  await createComponent(
    root,
    'demo',
    { name: 'Demo', permissions: ['notify'] },
    { 'index.html': '<h1>demo</h1>' },
  )
  const handler = createApiHandler(
    {},
    {
      customUi: new CustomUiService({ dataDir: directory, builtinComponents: [] }),
    },
  )
  const request = (method, pathname) =>
    new Promise((resolvePromise, rejectPromise) => {
      const chunks = []
      const res = {
        status: 0,
        headers: {},
        writeHead(status, headers = {}) {
          this.status = status
          Object.assign(this.headers, headers)
        },
        setHeader(name, value) {
          this.headers[name] = value
        },
        write(chunk) {
          chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk))
        },
        end(body) {
          if (body) chunks.push(Buffer.isBuffer(body) ? body : Buffer.from(body))
          resolvePromise({
            status: this.status,
            headers: this.headers,
            body: Buffer.concat(chunks).toString('utf8'),
          })
        },
        on() {},
        once() {},
        emit() {},
        removeListener() {},
        destroy() {},
        get headersSent() {
          return this.status !== 0
        },
      }
      void handler({ method, headers: {} }, res, new URL(`http://localhost${pathname}`)).then(
        (handled) => {
          if (!handled) rejectPromise(new Error(`route not handled: ${pathname}`))
        },
        rejectPromise,
      )
    })

  const list = await request('GET', '/api/custom-ui/components')
  assert.equal(list.status, 200)
  const payload = JSON.parse(list.body)
  assert.equal(payload.root, displayCustomUiPath(root))
  assert.equal(payload.components.length, 1)
  assert.equal(payload.components[0].id, 'demo')
  assert.equal(payload.components[0].directory, displayCustomUiPath(join(root, 'demo')))
  assert.deepEqual(payload.components[0].permissions, ['notify'])

  const entry = await request('GET', '/api/custom-ui/components/demo/assets/index.html')
  assert.equal(entry.status, 200)
  assert.match(entry.headers['Content-Type'], /text\/html/)
  assert.match(entry.body, /<h1>demo<\/h1>/)

  const nestedMissing = await request('GET', '/api/custom-ui/components/demo/assets/js/app.js')
  assert.equal(nestedMissing.status, 404)

  const traversal = await request('GET', '/api/custom-ui/components/demo/assets/..%2Fmanifest.json')
  assert.equal(traversal.status, 404)

  const bridge = await request('GET', '/api/custom-ui/bridge.js')
  assert.equal(bridge.status, 200)
  assert.match(bridge.headers['Content-Type'], /text\/javascript/)
  assert.match(bridge.body, /window\.pisper/)
})

test('组件凭证在桌面 Cookie 鉴权下加载资源，保持沙箱并拒绝越权、过期和撤销', async (t) => {
  const { createServer } = await import('node:http')
  const { createApiHandler } = await import('../http/api-handler.mjs')
  const { handleCustomUiResource } = await import('../http/routes/custom-ui.mjs')
  const { authorizeDesktopRequest } = await import('../desktop-sidecar-auth.mjs')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-auth-'))
  let now = 1
  const service = new CustomUiService({ dataDir: directory, now: () => now })
  const handler = createApiHandler({}, { customUi: service })
  await createComponent(
    service.root,
    'demo',
    { name: 'Demo' },
    {
      'index.html':
        '<script src="/api/custom-ui/bridge.js"></script><script type="module" src="./js/app.js"></script>',
      'js/app.js': 'export const result = 1',
      'image.svg': '<svg xmlns="http://www.w3.org/2000/svg"></svg>',
    },
  )
  await createComponent(
    service.root,
    'other',
    { name: 'Other' },
    { 'only-other.txt': 'not shared' },
  )
  let revoked = false
  const remoteAccess = {
    getDevice: () => ({ id: 'device', revokedAt: revoked ? 'now' : null }),
    trackResponse() {},
  }
  const server = createServer(async (req, res) => {
    const origin = `http://127.0.0.1:${server.address().port}`
    const url = new URL(req.url, origin)
    const json = (status, value) => {
      res.writeHead(status, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify(value))
    }
    if (await handleCustomUiResource(req, res, url, { customUi: service, remoteAccess, json }))
      return
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
  const authenticated = { Cookie: '__pisper_desktop=fixture', 'Content-Type': 'application/json' }
  const create = () =>
    fetch(`${origin}/api/custom-ui/components/demo/views`, {
      method: 'POST',
      headers: authenticated,
      body: JSON.stringify({ origin }),
    }).then((response) => response.json())
  assert.equal(
    (await fetch(`${origin}/api/custom-ui/components/demo/views`, { method: 'POST' })).status,
    401,
  )
  const view = await create()
  assert.match(view.id, /^[a-f0-9]{64}$/)
  const assetRoot = `/api/custom-ui/render/${view.id}/`
  const entry = await fetch(origin + view.entryUrl)
  assert.equal(entry.status, 200)
  const html = await entry.text()
  assert.ok(html.includes(`${origin}${assetRoot}bridge.js`))
  assert.ok(html.includes('./js/app.js'))
  const csp = entry.headers.get('Content-Security-Policy')
  assert.match(csp, /sandbox allow-scripts/)
  assert.doesNotMatch(csp, /allow-same-origin/)
  assert.ok(csp.includes(`connect-src ${origin}${assetRoot}`))
  assert.equal(entry.headers.get('Referrer-Policy'), 'no-referrer')
  for (const path of ['bridge.js', 'assets/js/app.js', 'assets/image.svg']) {
    const response = await fetch(origin + assetRoot + path)
    assert.equal(response.status, 200)
    assert.equal(response.headers.get('Access-Control-Allow-Origin'), '*')
    assert.match(response.headers.get('Content-Security-Policy'), /sandbox allow-scripts/)
    await response.text()
  }
  // 凭证不能变成会话认证，也不能读取另一个组件、隐藏文件或目录外符号链接。
  for (const path of [
    'assets/only-other.txt',
    'assets/..%2F..%2Fother%2Fonly-other.txt',
    'assets/manifest.json',
    'api/config',
  ]) {
    assert.equal((await fetch(origin + assetRoot + path)).status, 404)
  }
  assert.equal(
    (await fetch(origin + '/api/config', { headers: { Authorization: `Bearer ${view.id}` } }))
      .status,
    401,
  )
  assert.equal((await fetch(origin + view.entryUrl, { method: 'POST' })).status, 404)
  const legacy = await fetch(origin + '/api/custom-ui/components/demo/assets/index.html', {
    headers: authenticated,
  })
  assert.match(legacy.headers.get('Content-Security-Policy'), /sandbox allow-scripts/)
  await legacy.text()
  now += 4 * 60_000
  assert.equal(
    (
      await fetch(`${origin}/api/custom-ui/views/${view.id}`, {
        method: 'PUT',
        headers: authenticated,
      })
    ).status,
    200,
  )
  now += 4 * 60_000
  assert.equal((await fetch(origin + view.entryUrl)).status, 200)
  now += 6 * 60_000
  assert.equal((await fetch(origin + view.entryUrl)).status, 404)
  const fresh = await create()
  assert.equal(
    (
      await fetch(`${origin}/api/custom-ui/views/${fresh.id}`, {
        method: 'DELETE',
        headers: authenticated,
      })
    ).status,
    200,
  )
  assert.equal((await fetch(origin + fresh.entryUrl)).status, 404)
  const remote = await service.createView('demo', 'device', origin)
  assert.throws(() => service.renewView(remote.id, 'different-device'), /过期/)
  service.revokeView(remote.id, 'different-device')
  assert.equal((await fetch(origin + remote.entryUrl)).status, 200)
  revoked = true
  assert.equal((await fetch(origin + remote.entryUrl)).status, 404)
  const active = await create()
  service.dispose()
  assert.equal((await fetch(origin + active.entryUrl)).status, 404)
})

test('资产拒绝整个组件目录的符号链接逃逸及绝对路径', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-custom-ui-root-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new CustomUiService({ dataDir: directory })
  await createComponent(service.root, 'demo', { name: 'Demo' }, { 'index.html': '<p>ok</p>' })
  const outside = join(directory, 'outside')
  await mkdir(outside)
  await writeFile(join(outside, 'secret.txt'), 'fixture')
  await symlink(outside, join(service.root, 'alias'))
  assert.equal(await service.resolveAssetPath('alias', 'secret.txt'), null)
  assert.equal(await service.resolveAssetPath('demo', '/index.html'), null)
  await assert.rejects(
    service.createView('demo', 'local', 'https://example.invalid/;script-src *'),
    /来源/,
  )
})
