// 工作区 HTML 预览：能发令牌的范围、页面与相对路径资源能读到什么、响应头的隔离，以及鉴权前的接线。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises'
import { createServer, request } from 'node:http'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { HTML_PREVIEW_PREFIX, HtmlPreviewService } from '../services/html-preview-service.mjs'

async function fixture(t, options) {
  const base = await mkdtemp(join(tmpdir(), 'pisper-html-preview-'))
  t.after(() => rm(base, { recursive: true, force: true }))
  const workspace = join(base, 'workspace')
  const site = join(workspace, 'site')
  await mkdir(join(site, 'assets'), { recursive: true })
  await mkdir(join(site, '.git'))
  await writeFile(join(base, 'outside.html'), '<p>outside</p>')
  await writeFile(join(base, 'outside.txt'), 'outside')
  await writeFile(join(workspace, 'parent.css'), 'parent')
  await writeFile(join(workspace, 'index.html'), '<p>root</p>')
  await writeFile(join(site, 'index.html'), '<link rel="stylesheet" href="assets/a.css">')
  await writeFile(join(site, 'UPPER.HTM'), '<p>upper</p>')
  await writeFile(join(site, 'assets', 'a.css'), 'body{}')
  await writeFile(join(site, 'assets', 'a.js'), 'export {}')
  await writeFile(join(site, 'data.json'), '{}')
  await writeFile(join(site, 'notes.md'), '# notes')
  await writeFile(join(site, '.env'), 'SECRET=1')
  await writeFile(join(site, '.git', 'config.json'), '{}')
  await writeFile(join(site, 'secrets.json'), '{}')
  await writeFile(join(site, 'credentials.txt'), 'x')
  await writeFile(join(site, 'server.pem'), 'x')
  await symlink(join(base, 'outside.txt'), join(site, 'escape.txt'))
  await symlink(join(base, 'outside.html'), join(site, 'escape.html'))
  const service = new HtmlPreviewService(options)
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, `http://${req.headers.host}`)
    if (!(await service.handle(req, res, url))) {
      res.writeHead(418)
      res.end()
    }
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  t.after(() => new Promise((resolve) => server.close(resolve)))
  const origin = `http://127.0.0.1:${server.address().port}`
  return { base, workspace, site, service, origin }
}

const tokenOf = (url) => url.slice(HTML_PREVIEW_PREFIX.length).split('/')[0]

test('tokens are issued only for HTML files inside the workspace', async (t) => {
  const { base, workspace, service } = await fixture(t)
  const create = (path) => service.create({ workspace, path })
  const code = (path) =>
    create(path).then(
      () => 'ok',
      (error) => `${error.statusCode} ${error.code}`,
    )

  assert.match(
    (await create(join(workspace, 'site', 'index.html'))).url,
    /^\/_pisper\/html-preview\/[\w-]{43}\/index\.html$/,
  )
  assert.match((await create('site/index.html')).url, /\/index\.html$/)
  assert.match((await create('site/UPPER.HTM')).url, /\/UPPER\.HTM$/)

  assert.equal(await code(join(base, 'outside.html')), '403 html_preview_outside_workspace')
  assert.equal(await code('../outside.html'), '403 html_preview_outside_workspace')
  // 工作区内的符号链接指到外面，按实际位置判断。
  assert.equal(await code('site/escape.html'), '403 html_preview_outside_workspace')
  assert.equal(await code('site/missing.html'), '404 html_preview_not_found')
  assert.equal(await code('site'), '404 html_preview_not_found')
  assert.equal(await code('site/notes.md'), '400 html_preview_not_html')
  for (const invalid of [undefined, null, 42, {}, '', '   ', 'a\u0000.html', 'x'.repeat(4097)])
    assert.equal(await code(invalid), '400 html_preview_invalid_path', String(invalid).slice(0, 12))
})

test('the page and its relative assets load in an opaque origin without leaking the token', async (t) => {
  const { workspace, service, origin } = await fixture(t)
  const { url } = await service.create({ workspace, path: 'site/index.html' })
  const page = await fetch(origin + url)
  assert.equal(page.status, 200)
  assert.equal(page.headers.get('content-type'), 'text/html; charset=utf-8')
  const csp = page.headers.get('content-security-policy')
  assert.match(csp, /^sandbox\b/)
  assert.match(csp, /allow-scripts/)
  // 少了这两项隔离就失效：页面会和应用同源，或者能把应用整页导航走。
  assert.doesNotMatch(csp, /allow-same-origin|allow-top-navigation/)
  assert.equal(page.headers.get('referrer-policy'), 'no-referrer')
  assert.equal(page.headers.get('x-content-type-options'), 'nosniff')
  assert.equal(page.headers.get('cache-control'), 'no-store')
  assert.equal(await page.text(), '<link rel="stylesheet" href="assets/a.css">')

  const token = tokenOf(url)
  const asset = (path) => fetch(`${origin}${HTML_PREVIEW_PREFIX}${token}/${path}`)
  const css = await asset('assets/a.css')
  assert.equal(css.headers.get('content-type'), 'text/css; charset=utf-8')
  assert.equal(await css.text(), 'body{}')
  // 不透明来源加载模块脚本要过 CORS；响应同样带 sandbox，直接打开也不会同源。
  const js = await asset('assets/a.js')
  assert.equal(js.headers.get('access-control-allow-origin'), '*')
  assert.match(js.headers.get('content-security-policy'), /^sandbox\b/)
  assert.equal((await asset('data.json')).status, 200)

  const head = await fetch(origin + url, { method: 'HEAD' })
  assert.equal(head.status, 200)
  assert.equal(await head.text(), '')
  assert.equal((await fetch(origin + url, { method: 'POST' })).status, 405)
})

test('a token cannot read outside its directory, hidden files, or files that look like credentials', async (t) => {
  const { workspace, service, origin } = await fixture(t)
  const { url } = await service.create({ workspace, path: 'site/index.html' })
  const token = tokenOf(url)
  const blocked = [
    '.env',
    '.git/config.json',
    'secrets.json',
    'credentials.txt',
    // 不在网页资源类型里。
    'server.pem',
    'notes.md',
    // 符号链接指到目录外。
    'escape.txt',
    'escape.html',
    // 令牌只对 HTML 所在目录有效，上一级和工作区根都不行。
    '../parent.css',
    '..%2Fparent.css',
    '%2e%2e/parent.css',
    '%2e%2e%2fparent.css',
    // 编码的分隔符不能把点开头的段藏进一个段里。
    'x%2F..%2F.git%2Fconfig.json',
    'assets%2Fa.css',
    '..%5cparent.css',
    'assets/../../parent.css',
    'assets//a.css',
    'assets/',
    '',
    'bad%zz.css',
    'a%00.css',
  ]
  for (const path of blocked) {
    // fetch 会折叠 ..，这里要原样发出去。
    const response = await new Promise((resolve, reject) => {
      const { hostname, port } = new URL(origin)
      const req = request(
        { hostname, port, path: `${HTML_PREVIEW_PREFIX}${token}/${path}` },
        (res) => {
          res.resume()
          resolve(res)
        },
      )
      req.on('error', reject)
      req.end()
    })
    assert.equal(response.statusCode, 404, path)
  }
})

test('unknown, expired and evicted tokens are refused, and other paths are left alone', async (t) => {
  let now = 1000
  const { workspace, service, origin } = await fixture(t, {
    now: () => now,
    tokenTtlMs: 100,
    maxTokens: 2,
  })
  assert.equal((await fetch(`${origin}${HTML_PREVIEW_PREFIX}nope/index.html`)).status, 404)
  assert.equal((await fetch(`${origin}/api/sessions`)).status, 418)
  assert.equal((await fetch(`${origin}/_pisper/html-preview`)).status, 418)

  const first = await service.create({ workspace, path: 'site/index.html' })
  const second = await service.create({ workspace, path: 'site/index.html' })
  // 每次点开都换地址，iframe 才会重新加载改过的页面。
  assert.notEqual(first.url, second.url)
  assert.equal((await fetch(origin + first.url)).status, 200)
  now += 101
  assert.equal((await fetch(origin + first.url)).status, 404)

  const third = await service.create({ workspace, path: 'site/index.html' })
  const fourth = await service.create({ workspace, path: 'site/index.html' })
  assert.equal((await fetch(origin + third.url)).status, 200)
  const fifth = await service.create({ workspace, path: 'site/index.html' })
  assert.equal((await fetch(origin + third.url)).status, 404)
  assert.equal((await fetch(origin + fourth.url)).status, 200)
  assert.equal((await fetch(origin + fifth.url)).status, 200)
  assert.equal(service.tokens.size, 2)
})

test('the route runs before authentication, and sandboxed pages cannot write to the API', async () => {
  const source = await readFile('runtime/app-runtime.mjs', 'utf8')
  const handled = source.indexOf('await htmlPreviews.handle(req, res, url)')
  const originGuard = source.indexOf("req.headers.origin === 'null'")
  const remote = source.indexOf('authorizeRemoteRequest(req, res, url, { remoteAccess })')
  const proxy = source.indexOf('isComponentProxyRequest(req)')
  assert.ok(proxy > 0 && handled > proxy, '组件代理的请求不能打到预览路由')
  assert.ok(originGuard > handled && remote > originGuard)
  // iframe 不靠响应头这一层：代理丢了响应头也不能和应用同源。
  const preview = await readFile('src/lib/web-preview.ts', 'utf8')
  assert.doesNotMatch(preview.match(/HTML_PREVIEW_SANDBOX = '([^']+)'/)[1], /allow-same-origin/)
  const pane = await readFile('src/features/chat/SessionBrowserPane.tsx', 'utf8')
  assert.match(pane, /isHtmlPreviewUrl\(url\) \? \{ sandbox: HTML_PREVIEW_SANDBOX \}/)
})
