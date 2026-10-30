// 会话做主页的冒烟（第 4 期）：真实生产构建（dist）+ 进程内隔离 Runtime，数据目录全新（没有任何会话），
// 会话目录全部来自真实 Runtime，所以“落地主页不建会话”可以直接数 GET /api/sessions。
// 只在浏览器边界注入一个已配置的模型（启动台需要），并把 /api/chat 换成一个最小的 run 头帧，不连接模型。
// 覆盖：全新数据目录打开 / → 新任务草稿且会话数不变；发出第一条消息才建会话并以 replace 换成 /board/:id；
// 重新载入 / → 回到上次停留的会话；泳道条“打开看板”入口与 ⇧⌘B；“会话”标签；未知路径与 /chat 回主页；
// 归档该会话后 / → 草稿；桌面标签顺序与点亮；会话目录加载中 / 失败时的主页状态。
// 手机主页（草稿、返回根、标签栏、更多里的组件磁贴）在 smoke-mobile-shell 里覆盖。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = await mkdtemp(join(tmpdir(), 'pisper-session-home-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
const fixtureHome = join(output, 'home')
await mkdir(workspace)
await mkdir(fixtureHome)
process.env.HOME = fixtureHome
process.env.USERPROFILE = fixtureHome
process.env.CODEX_HOME = join(fixtureHome, '.codex')
process.env.CLAUDE_CONFIG_DIR = join(fixtureHome, '.claude')
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace
const { createPisperRuntime } = await import('../runtime/app-runtime.mjs')
const app = await createPisperRuntime({
  root,
  frontendRoot: join(root, 'dist'),
  runtimeCwd: workspace,
  dataDir,
  production: true,
  port: 0,
  host: '127.0.0.1',
  remote: { enabled: false },
})
await app.initialized
const base = app.url
const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
const report = { status: 'running', checks: [], pageErrors: [] }
let browser
let failurePage

async function sessionCount() {
  const list = await fetch(`${base}/api/sessions`).then((response) => response.json())
  return (list.sessions || []).length
}

async function newContext(options) {
  const context = await browser.newContext({ locale: 'zh-CN', ...options })
  await context.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  await context.route('**/api/app-update*', (route) =>
    route.fulfill({ json: { state: 'current', checkedAt: Date.now() } }),
  )
  // 注入一个已配置的对话模型，让草稿的启动台可以发送；真实配置的其余字段原样保留。
  await context.route(/\/api\/config(\?.*)?$/, async (route) => {
    if (route.request().method() !== 'GET') return route.continue()
    const response = await route.fetch()
    const data = await response.json().catch(() => ({}))
    await route.fulfill({
      response,
      json: {
        ...data,
        provider: 'openai',
        model: 'gpt-5.6-luna',
        providers: [
          {
            id: 'openai',
            name: 'OpenAI',
            configured: true,
            enabled: true,
            models: [
              {
                id: 'gpt-5.6-luna',
                name: 'GPT-5.6 Luna',
                kind: 'chat',
                thinkingLevels: ['off', 'low', 'medium', 'high'],
              },
            ],
          },
        ],
      },
    })
  })
  // 第一条消息：记录请求体，用最小的 run 头帧应答（不连接模型）。
  const chats = []
  await context.route(/\/api\/chat(\?.*)?$/, async (route) => {
    const body = JSON.parse(route.request().postData() || '{}')
    chats.push(body)
    const header = { runId: 'run-home', kind: 'chat', sessionId: body.sessionId, cursor: 0 }
    await route.fulfill({
      status: 200,
      headers: { 'Content-Type': 'text/event-stream' },
      body: `event: run\ndata: ${JSON.stringify(header)}\n\nevent: done\ndata: {}\n\n`,
    })
  })
  // 建会话的请求：主页落地与导航都不得产生 POST /api/sessions。
  const creates = []
  context.on('request', (request) => {
    const url = new URL(request.url())
    if (request.method() === 'POST' && url.pathname === '/api/sessions') creates.push(url.pathname)
  })
  return { context, chats, creates }
}

async function openPage(context, label, hash) {
  const page = await context.newPage()
  failurePage = page
  page.setDefaultTimeout(15000)
  page.on('pageerror', (error) => report.pageErrors.push(`${label}: ${error.message}`))
  await page.goto(`${base}/${hash}`)
  return page
}

const hashOf = (page) => new URL(page.url()).hash
const activeTabs = (page) =>
  page.evaluate(() =>
    [...document.querySelectorAll('[data-app-titlebar] [data-shell-tab][aria-current="page"]')].map(
      (tab) => tab.dataset.shellTab,
    ),
  )
const sessionRoute = /^#\/board\/(?!new$)[^/]+$/

try {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  assert.equal(await sessionCount(), 0, 'the data directory starts without sessions')

  // 1. 全新数据目录打开 / ：新任务草稿，不建会话；“会话”标签点亮，标签顺序 会话 / 看板 / 组件 / 更多 / 设置。
  const { context, chats, creates } = await newContext({
    viewport: { width: 1440, height: 900 },
    colorScheme: 'light',
  })
  const page = await openPage(context, 'desktop home', '#/')
  await page.locator('[data-new-task-draft="desktop"]').waitFor({ timeout: 60000 })
  assert.equal(await page.locator('[data-session-home="draft"]').count(), 1)
  assert.equal(hashOf(page), '#/')
  assert.deepEqual(
    await page
      .locator('[data-app-titlebar] [data-shell-tab]')
      .evaluateAll((tabs) => tabs.map((tab) => tab.dataset.shellTab)),
    ['session', 'board', 'components', 'more', 'settings'],
  )
  assert.deepEqual(await activeTabs(page), ['session'])
  await page.locator('[data-board-entry]').waitFor()
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-launch-input'))
  await page.waitForTimeout(1500)
  assert.equal(await sessionCount(), 0, 'landing on the home creates no session')
  assert.deepEqual(creates, [])
  await page.screenshot({ path: join(output, 'home-draft.png') })
  // 看板标签点亮看板；“会话”标签回到主页（仍是草稿）。
  await page.locator('[data-app-titlebar] [data-shell-tab="board"]').click()
  await page.waitForURL((url) => url.hash === '#/board')
  await page.locator('[data-board-root]').waitFor()
  assert.deepEqual(await activeTabs(page), ['board'])
  await page.locator('[data-app-titlebar] [data-shell-tab="session"]').click()
  await page.waitForURL((url) => url.hash === '#/')
  await page.locator('[data-new-task-draft="desktop"]').waitFor()
  assert.deepEqual(await activeTabs(page), ['session'])
  assert.equal(await sessionCount(), 0)
  assert.deepEqual(creates, [])
  report.checks.push(
    'Fresh data directory: / shows the new-task draft (URL stays /, 会话 lit, composer focused, 看板 entry on the strip) and GET /api/sessions stays at 0 with no POST; the 看板 tab lights 看板 and the 会话 tab returns to the draft',
  )

  // 2. 发出第一条消息才建会话，以 replace 换成 /board/:id（标为主页落地），浏览器后退不回到草稿。
  await page.locator('[data-launch-input]').fill('只是测试：列出当前目录')
  await page.keyboard.press('Enter')
  await page.waitForURL((url) => sessionRoute.test(url.hash), { timeout: 30000 })
  const id = decodeURIComponent(hashOf(page).split('/').pop())
  await page.locator('[data-session-header]').first().waitFor({ timeout: 30000 })
  assert.equal(creates.length, 1, 'the first message creates exactly one session')
  assert.equal(await sessionCount(), 1)
  assert.equal(chats.length, 1)
  assert.equal(chats[0].sessionId, id)
  assert.match(chats[0].message, /列出当前目录/)
  assert.equal(await page.evaluate(() => window.history.state?.usr?.navOrigin), 'home')
  assert.deepEqual(await activeTabs(page), ['session'])
  await page.waitForFunction(
    (expected) => localStorage.getItem('pisper-last-session') === expected,
    id,
  )
  await page.goBack()
  await page.waitForURL((url) => url.hash === '#/board')
  await page.goForward()
  await page.waitForURL((url) => url.hash === `#/board/${id}`)
  report.checks.push(
    'The first message from the draft creates exactly one session, sends to it, replaces / with /board/:id marked as a home landing (会话 lit) and records it as the last session; browser back skips the replaced draft',
  )

  // 3. 重新载入 / → 回到上次停留的会话。
  await page.goto(`${base}/#/`)
  await page.reload()
  await page.waitForURL((url) => url.hash === `#/board/${id}`, { timeout: 30000 })
  await page.locator('[data-session-header]').first().waitFor({ timeout: 30000 })
  assert.equal(await page.evaluate(() => window.history.state?.usr?.navOrigin), 'home')
  assert.deepEqual(await activeTabs(page), ['session'])
  assert.equal(creates.length, 1)
  // 泳道条最左侧是“打开看板”入口（push，后退回会话），⇧⌘B 打开看板，“会话”标签回到上次的会话。
  const entry = page.locator('[data-board-entry]')
  assert.match(await entry.getAttribute('aria-label'), /^打开看板（.*B）$/)
  assert.equal(await entry.getAttribute('aria-keyshortcuts'), `${mod}+Shift+B`)
  await entry.click()
  await page.waitForURL((url) => url.hash === '#/board')
  await page.goBack()
  await page.waitForURL((url) => url.hash === `#/board/${id}`)
  await page.locator('[data-session-header]').first().waitFor()
  await page.locator('[data-board-entry]').waitFor()
  await page.locator('.focus-composer textarea').first().click()
  await page.keyboard.press(`${mod}+Shift+KeyB`)
  await page.waitForURL((url) => url.hash === '#/board')
  await page.locator('[data-app-titlebar] [data-shell-tab="session"]').click()
  await page.waitForURL((url) => url.hash === `#/board/${id}`)
  // 未知路径与 /chat 兼容入口回落到主页（上次停留的会话）。
  for (const path of ['#/does-not-exist', '#/chat']) {
    await page.goto(`${base}/${path}`)
    await page.waitForURL((url) => url.hash === `#/board/${id}`, { timeout: 30000 })
  }
  assert.equal(creates.length, 1, 'navigation never creates sessions')
  report.checks.push(
    'Reloading / lands on the last session; the lane strip 看板 entry announces ⇧⌘B, pushes /board and browser back returns; ⇧⌘B opens the board from the composer; the 会话 tab, unknown paths and /chat all return to the last session',
  )

  // 4. 归档该会话后回到主页草稿；重新载入 / 仍是草稿。
  await page.locator('[data-session-header]').first().waitFor()
  await page.locator(`[id="task-${id}-more"]`).click()
  await page.getByRole('menuitem', { name: '归档', exact: true }).click()
  await page.waitForURL((url) => url.hash === '#/', { timeout: 15000 })
  await page.locator('[data-new-task-draft="desktop"]').waitFor()
  assert.equal(await page.evaluate(() => localStorage.getItem('pisper-last-session')), null)
  await page.reload()
  await page.locator('[data-new-task-draft="desktop"]').waitFor({ timeout: 30000 })
  assert.equal(hashOf(page), '#/')
  assert.equal(creates.length, 1)
  await context.close()
  report.checks.push(
    'Archiving the current session returns to the home draft and forgets it; reloading / stays on the draft',
  )

  // 4b. 主页的页面 id 也是 session，但那里没有会话路由接收“打开 / 新建”请求：外壳必须自己导航。
  // 派发浮层 ⌘↵ 与 ⌘K 选会话都要换成 /board/:id，且不留下过期的 pisper-session-open-request。
  {
    const clearLastSession = async () => {
      const response = await fetch(`${base}/api/local/browser-preferences`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          updates: { 'pisper-last-session': null },
          revisions: { 'pisper-last-session': Date.now() * 1000 + 999 },
        }),
      })
      assert.ok(response.ok, `clear the last session: ${response.status}`)
    }
    const created = await fetch(`${base}/api/sessions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: '主页打开会话夹具' }),
    })
    assert.equal(created.status, 201)
    const target = (await created.json()).id
    assert.ok(target)
    const sessionHeader = '[data-session-header]:not([data-session-header="draft"])'

    // 派发浮层（⌘N）叠在草稿上，⌘↵ 派发并打开该会话。
    await clearLastSession()
    const dispatchRun = await newContext({ viewport: { width: 1440, height: 900 } })
    const dispatchPage = await openPage(dispatchRun.context, 'home dispatch', '#/')
    await dispatchPage.locator('[data-new-task-draft="desktop"]').waitFor({ timeout: 60000 })
    await dispatchPage.waitForFunction(() =>
      document.activeElement?.hasAttribute('data-launch-input'),
    )
    await dispatchPage.keyboard.press(`${mod}+KeyN`)
    const overlayInput = dispatchPage.locator('[data-launch-console="overlay"] [data-launch-input]')
    await overlayInput.waitFor({ timeout: 15000 })
    await overlayInput.fill('只是测试：从主页的派发浮层打开')
    await dispatchPage.keyboard.press(`${mod}+Enter`)
    await dispatchPage.waitForURL((url) => sessionRoute.test(url.hash), { timeout: 30000 })
    // 懒加载的会话路由接手前，旧的草稿页还会短暂留在页面上：等它换成真正的会话页头。
    await dispatchPage.locator(sessionHeader).first().waitFor({ timeout: 30000 })
    assert.equal(dispatchRun.creates.length, 1, 'the overlay creates exactly one session')
    assert.equal(await dispatchPage.locator('[data-new-task-draft]').count(), 0)
    await dispatchPage.waitForFunction(
      () => localStorage.getItem('pisper-session-open-request') === null,
    )
    await dispatchRun.context.close()

    // ⌘K 选中一个会话。
    await clearLastSession()
    const paletteRun = await newContext({ viewport: { width: 1440, height: 900 } })
    const palettePage = await openPage(paletteRun.context, 'home palette', '#/')
    await palettePage.locator('[data-new-task-draft="desktop"]').waitFor({ timeout: 60000 })
    const paletteInput = palettePage.locator('input[role="combobox"]')
    for (let attempt = 0; attempt < 5 && !(await paletteInput.isVisible()); attempt += 1) {
      await palettePage.keyboard.press(`${mod}+KeyK`)
      await paletteInput.waitFor({ timeout: 3000 }).catch(() => {})
    }
    await paletteInput.fill('主页打开会话夹具')
    const option = palettePage.getByRole('option', { name: /主页打开会话夹具/ }).first()
    await option.waitFor({ timeout: 15000 })
    await option.click()
    await palettePage.waitForURL((url) => url.hash === `#/board/${encodeURIComponent(target)}`, {
      timeout: 30000,
    })
    await palettePage.locator(sessionHeader).first().waitFor({ timeout: 30000 })
    assert.equal(await palettePage.locator('[data-new-task-draft]').count(), 0)
    await palettePage.waitForFunction(
      () => localStorage.getItem('pisper-session-open-request') === null,
    )
    assert.deepEqual(paletteRun.creates, [])
    await paletteRun.context.close()
  }
  report.checks.push(
    'On the home draft the shell navigates by itself: the ⌘N dispatch overlay ⌘↵ opens the new session and ⌘K → session opens /board/:id, leaving no stale pisper-session-open-request',
  )

  // 5. 记着上次的会话时，主页要先等会话目录核对它：加载中不闪草稿、不跳看板；记着的会话已归档 → 草稿；
  // 目录读取失败时给出错误态、重试与“开始新任务”。“上次停留”经偏好镜像按设备同步，这里直接写进镜像。
  {
    const response = await fetch(`${base}/api/local/browser-preferences`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        updates: { 'pisper-last-session': id },
        revisions: { 'pisper-last-session': Date.now() * 1000 + 999 },
      }),
    })
    assert.ok(response.ok, `seed the last session: ${response.status}`)
    const { context } = await newContext({ viewport: { width: 1440, height: 900 } })
    let release
    const gate = new Promise((done) => (release = done))
    let fail = false
    await context.route(/\/api\/sessions(\?.*)?$/, async (route) => {
      if (route.request().method() !== 'GET') return route.continue()
      if (fail) return route.fulfill({ status: 500, json: { error: 'fixture failure' } })
      await gate
      return route.continue()
    })
    const loading = await openPage(context, 'home loading', '#/')
    await loading.locator('[data-session-home="loading"]').waitFor({ timeout: 60000 })
    await loading.waitForTimeout(500)
    assert.equal(await loading.locator('[data-new-task-draft]').count(), 0)
    assert.equal(hashOf(loading), '#/')
    release()
    await loading.locator('[data-new-task-draft="desktop"]').waitFor({ timeout: 30000 })
    await loading.close()
    fail = true
    const failed = await openPage(context, 'home error', '#/')
    await failed.locator('[data-session-home="error"]').waitFor({ timeout: 60000 })
    assert.equal(await failed.getByRole('button', { name: '重试', exact: true }).count(), 1)
    await failed.screenshot({ path: join(output, 'home-error.png') })
    await failed.getByRole('button', { name: '开始新任务', exact: true }).click()
    await failed.locator('[data-new-task-draft="desktop"]').waitFor()
    await context.close()
  }
  report.checks.push(
    'With a remembered session the home waits on the catalog (loading state, no draft flash, no jump to the board) and shows the draft once the remembered session turns out archived; a failed catalog shows an error with 重试 and an explicit 开始新任务',
  )

  assert.deepEqual(report.pageErrors, [])
  report.status = 'passed'
  console.log(JSON.stringify(report, null, 2))
} catch (error) {
  report.status = 'failed'
  report.failure = String(error?.stack || error)
  await failurePage?.screenshot({ path: join(output, 'failure.png') }).catch(() => {})
  throw error
} finally {
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  await app.close()
  for (const directory of [dataDir, workspace, fixtureHome]) {
    assert.equal(dirname(directory), output, 'Unsafe temporary cleanup path')
    await rm(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
  console.log(`Session home smoke report and screenshots: ${output}`)
}
