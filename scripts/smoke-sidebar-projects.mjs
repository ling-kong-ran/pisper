// 真实侧栏、目录偏好与页面状态恢复；API 全部隔离，不接触开发者的会话或浏览器配置。
import assert from 'node:assert/strict'
import { mkdir, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const output = join(root, 'release/sidebar-projects-smoke')
await mkdir(output, { recursive: true })
const bundled = await build({
  entryPoints: [join(root, 'scripts/fixtures/sidebar-projects.jsx')],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  loader: { '.css': 'empty' },
  logLevel: 'warning',
})
const fixture = bundled.outputFiles[0].text
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<style>body{margin:20px;font:14px sans-serif}#root{width:360px}#sidebar-recent-sessions{min-height:260px}button{min-height:28px}svg{width:16px;height:16px}[role=menu]{background:white;border:1px solid #999;padding:8px}[role=menuitem]{padding:8px}small{display:none}</style>
</head><body><div id="root"></div><script type="module" src="/fixture.js"></script></body></html>`
const initial = [
  { id: 'a', name: 'Alpha chat', cwd: '/fixture/alpha', modified: '2026-01-03T00:00:00Z' },
  { id: 'b', name: 'Beta chat', cwd: '/fixture/beta', modified: '2026-01-02T00:00:00Z' },
  { id: 'c', name: 'Beta second', cwd: '/fixture/beta', modified: '2026-01-01T00:00:00Z' },
]
let sessions = structuredClone(initial)
// 兼容只有 order/names 的旧 v1 偏好；空项目也应直接恢复。
let snapshot = {
  version: 1,
  values: {
    'pisper-workspace-order': JSON.stringify({
      state: {
        order: ['/fixture/beta', '/fixture/empty', '/fixture/alpha'],
        names: { '/fixture/empty': 'Saved empty' },
      },
      version: 1,
    }),
  },
  revisions: {},
}
const report = { checks: [], pageErrors: [], deniedRequests: [] }
const deletions = []
let failDelete = ''
let activePage
const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
const browser = await chromium.launch({ executablePath: resolve(executablePath), headless: true })
const preferences = () => JSON.parse(snapshot.values['pisper-workspace-order']).state

async function setup(port) {
  const origin = `http://127.0.0.1:${port}`
  const context = await browser.newContext({
    locale: 'zh-CN',
    viewport: { width: 1100, height: 900 },
    serviceWorkers: 'block',
  })
  await context.addInitScript(() => {
    window.sidebarFixture = {
      creations: [],
      navigation: [],
      confirmations: [],
      notices: [],
      approve: true,
      nextName: null,
    }
  })
  const page = await context.newPage()
  activePage = page
  page.setDefaultTimeout(4000)
  page.on('pageerror', (error) => report.pageErrors.push(error.message))
  await page.route('**/*', (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (url.origin !== origin) {
      report.deniedRequests.push(`${request.method()} ${url.pathname}`)
      return route.abort('blockedbyclient')
    }
    if (url.pathname === '/') return route.fulfill({ contentType: 'text/html', body: html })
    if (url.pathname === '/fixture.js')
      return route.fulfill({ contentType: 'text/javascript', body: fixture })
    if (url.pathname === '/favicon.ico') return route.fulfill({ status: 204 })
    if (url.pathname === '/api/local/browser-preferences') {
      if (request.method() === 'GET') return route.fulfill({ json: snapshot })
      const { updates, revisions } = request.postDataJSON()
      for (const [key, value] of Object.entries(updates)) {
        if (revisions[key] > (snapshot.revisions[key] || 0)) {
          snapshot.values[key] = value
          snapshot.revisions[key] = revisions[key]
        }
      }
      return route.fulfill({ json: {} })
    }
    if (url.pathname === '/api/sessions' && request.method() === 'GET')
      return route.fulfill({ json: { sessions } })
    if (url.pathname.startsWith('/api/sessions/') && request.method() === 'DELETE') {
      const id = decodeURIComponent(url.pathname.split('/').at(-1))
      if (id === failDelete)
        return route.fulfill({ status: 409, json: { error: 'Fixture busy session' } })
      deletions.push(id)
      sessions = sessions.filter((session) => session.id !== id)
      return route.fulfill({ json: { ok: true } })
    }
    report.deniedRequests.push(`${request.method()} ${url.pathname}`)
    return route.abort('blockedbyclient')
  })
  await page.goto(origin)
  await page.getByRole('button', { name: '新建项目', exact: true }).waitFor()
  return { page, context }
}

const project = (page, name) =>
  page.locator('#sidebar-recent-sessions button[aria-expanded]').filter({ hasText: name })
function projectNames(page) {
  return page.locator('#sidebar-recent-sessions button[aria-expanded] > span').allTextContents()
}
async function expectCollapsed(page, expected) {
  assert.deepEqual(
    await page
      .locator('#sidebar-recent-sessions button[aria-expanded]')
      .evaluateAll((buttons) => buttons.map((button) => button.getAttribute('aria-expanded'))),
    expected,
  )
}
async function openProjectDelete(page, name) {
  const confirmations = await page.evaluate(() => window.sidebarFixture.confirmations.length)
  await project(page, name).click({ button: 'right' })
  await page.getByRole('menuitem', { name: '删除项目', exact: true }).click()
  await page.waitForFunction(
    (count) => window.sidebarFixture.confirmations.length > count,
    confirmations,
  )
}
async function flushPreference(page, predicate) {
  // 等待真实的 Runtime 偏好镜像，不用额外延时推测后台写入完成。
  await page.waitForFunction(async (predicateSource) => {
    const value = await fetch('/api/local/browser-preferences').then((response) => response.json())
    const state = JSON.parse(value.values['pisper-workspace-order']).state
    return new Function('state', `return (${predicateSource})(state)`)(state)
  }, predicate.toString())
}

try {
  const first = await setup(45874)
  const page = first.page
  await page.getByRole('button', { name: /Alpha chat/ }).waitFor()
  assert.deepEqual(await projectNames(page), ['beta', 'Saved empty', 'alpha'])
  await expectCollapsed(page, ['true', 'true', 'true'])
  report.checks.push(
    'old order/names restore empty projects in the saved order, initially expanded',
  )

  const collapse = page.getByRole('button', { name: '收起所有项目', exact: true })
  await collapse.focus()
  await page.keyboard.press('Enter')
  await page.getByRole('button', { name: '展开所有项目', exact: true }).waitFor()
  await expectCollapsed(page, ['false', 'false', 'false'])
  assert.equal(await page.getByRole('button', { name: /Alpha chat/ }).count(), 0)
  assert.equal(
    await page
      .getByRole('button', { name: '展开所有项目', exact: true })
      .getAttribute('aria-controls'),
    'sidebar-recent-sessions',
  )
  await flushPreference(page, (state) => state.collapsed?.length === 3)
  await page.reload()
  await page.getByRole('button', { name: '展开所有项目', exact: true }).waitFor()
  await expectCollapsed(page, ['false', 'false', 'false'])
  report.checks.push('collapse all works with Enter, reports aria state and survives reload')

  await first.context.close()
  const second = await setup(45875)
  const next = second.page
  await next.getByRole('button', { name: '展开所有项目', exact: true }).waitFor()
  await expectCollapsed(next, ['false', 'false', 'false'])
  await next.getByRole('button', { name: '展开所有项目', exact: true }).focus()
  await next.keyboard.press('Space')
  await next.getByRole('button', { name: /Alpha chat/ }).waitFor()
  await expectCollapsed(next, ['true', 'true', 'true'])
  report.checks.push(
    'fresh browser context on another Runtime port restores collapsed projects and Space expands all',
  )

  await next.getByRole('button', { name: /Alpha chat/ }).click({ button: 'right' })
  await next.getByRole('menuitem', { name: '删除', exact: true }).click()
  await next.getByRole('button', { name: /Alpha chat/ }).waitFor({ state: 'detached' })
  assert.deepEqual(await projectNames(next), ['beta', 'Saved empty', 'alpha'])
  await project(next, 'alpha').waitFor()
  assert(deletions.includes('a'))
  // 与历史页批量删除相同的会话删除事件，侧栏不能因此清理目录记录。
  sessions = []
  await next.evaluate(() => window.sidebarFixture.deleted({ deletedIds: ['b', 'c'] }))
  await next.getByRole('button', { name: /Beta chat/ }).waitFor({ state: 'detached' })
  assert.deepEqual(await projectNames(next), ['beta', 'Saved empty', 'alpha'])
  await next.reload()
  await project(next, 'alpha').waitFor()
  assert.deepEqual(await projectNames(next), ['beta', 'Saved empty', 'alpha'])
  report.checks.push(
    'single and batch deletion of final chats retain projects, including after restart',
  )

  await next.getByRole('button', { name: '收起所有项目', exact: true }).click()
  await next.getByRole('button', { name: '在 /fixture/alpha 中新建会话', exact: true }).click()
  assert.deepEqual(await next.evaluate(() => window.sidebarFixture.creations), [
    { cwd: '/fixture/alpha' },
  ])
  assert.equal(await project(next, 'alpha').getAttribute('aria-expanded'), 'true')
  assert.equal(await project(next, 'beta').getAttribute('aria-expanded'), 'false')
  report.checks.push(
    'new chat from an empty project retains its directory and expands only that project',
  )

  await next.getByRole('button', { name: '筛选会话', exact: true }).click()
  const search = next.getByRole('textbox', { name: '搜索会话', exact: true })
  await search.fill('Saved empty')
  assert.deepEqual(await projectNames(next), ['Saved empty'])
  await next.getByRole('button', { name: '收起所有项目', exact: true }).click()
  await search.fill('')
  await expectCollapsed(next, ['false', 'false', 'false'])
  await next.getByRole('button', { name: '展开所有项目', exact: true }).click()
  await search.fill('/fixture/alpha')
  assert.deepEqual(await projectNames(next), ['alpha'])
  await search.fill('missing-project')
  await next.getByText('没有匹配的会话', { exact: true }).waitFor()
  await search.fill('')
  assert.deepEqual(await projectNames(next), ['beta', 'Saved empty', 'alpha'])
  report.checks.push(
    'empty projects match custom names and paths; search retains order and bulk collapse includes filtered projects',
  )

  await next.evaluate(() => {
    window.sidebarFixture.approve = false
  })
  await openProjectDelete(next, 'Saved empty')
  assert.deepEqual(await projectNames(next), ['beta', 'Saved empty', 'alpha'])
  await next.evaluate(() => {
    window.sidebarFixture.approve = true
  })
  await openProjectDelete(next, 'Saved empty')
  await project(next, 'Saved empty').waitFor({ state: 'detached' })
  await flushPreference(
    next,
    (state) => !state.order.includes('/fixture/empty') && !state.names['/fixture/empty'],
  )
  assert.match(
    (await next.evaluate(() => window.sidebarFixture.confirmations.at(-1))).message,
    /0 个会话/,
  )
  await next.reload()
  await project(next, 'alpha').waitFor()
  assert.deepEqual(await projectNames(next), ['beta', 'alpha'])
  report.checks.push(
    'explicit empty-project deletion still requires confirmation and removes saved project/name across restart',
  )

  sessions = structuredClone(initial.filter((session) => session.cwd === '/fixture/beta'))
  await next.evaluate(() => window.sidebarFixture.refresh())
  await next.getByRole('button', { name: /Beta chat/ }).waitFor()
  failDelete = 'c'
  await openProjectDelete(next, 'beta')
  await next.waitForFunction(() =>
    window.sidebarFixture.notices.some((notice) => notice.tone === 'error'),
  )
  await project(next, 'beta').waitFor()
  assert(preferences().order.includes('/fixture/beta'))
  failDelete = ''
  await openProjectDelete(next, 'beta')
  await project(next, 'beta').waitFor({ state: 'detached' })
  await flushPreference(next, (state) => !state.order.includes('/fixture/beta'))
  assert.deepEqual(await projectNames(next), ['alpha'])
  report.checks.push(
    'partially failed project deletion retains project; successful retry removes it',
  )

  await next.screenshot({ path: join(output, 'empty-project.png') })
  await second.context.close()
  assert.deepEqual(report.pageErrors, [])
  assert.deepEqual(report.deniedRequests, [])
  console.log(`PASS: ${report.checks.length} sidebar project interaction checks`)
} catch (error) {
  if (activePage && !activePage.isClosed())
    await activePage.screenshot({ path: join(output, 'failure.png') })
  report.failure = error.stack || String(error)
  throw error
} finally {
  await browser.close()
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
}
