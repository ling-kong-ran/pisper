import assert from 'node:assert/strict'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { chromium } from 'playwright-core'

const output = await mkdtemp(join(tmpdir(), 'pisper-replica-shell-'))
const server = await createServer({
  configFile: false,
  cacheDir: join(output, 'vite-cache'),
  plugins: [
    react(),
    tailwindcss(),
    {
      name: 'replica-shell-fixture',
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          if (request.url.split('?')[0] !== '/__shell') return next()
          response.setHeader('Content-Type', 'text/html')
          response.end(
            await server.transformIndexHtml(
              '/__shell',
              '<!doctype html><html lang="zh-CN"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/replica-shell.tsx"></script></body></html>',
            ),
          )
        })
      },
    },
  ],
  resolve: { alias: { '@': `${process.cwd()}/src`, '@shared': `${process.cwd()}/shared` } },
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
})
let browser
try {
  await server.listen()
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : {}),
  })
  const page = await browser.newPage({
    viewport: { width: 1200, height: 800 },
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    deviceScaleFactor: 1,
  })
  // 固定相对时间文案，但不冻结定时器，让防抖、动画与请求时序保持真实。
  await page.clock.setFixedTime(new Date('2026-10-20T09:00:00Z'))
  page.setDefaultTimeout(6000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const sessions = [
    {
      id: 'alpha',
      name: 'Alpha conversation',
      firstMessage: 'fixture',
      modified: '2026-10-20T00:00:00Z',
    },
    { id: 'fail', name: 'Failure conversation', modified: '2026-10-19T00:00:00Z' },
  ]
  // 手机标签栏的看板徽标：两个失败的会话落在“需要你”（不依赖 /live 的审批详情）。
  const phoneSessions = [
    ...sessions,
    ...['first', 'second'].map((id) => ({
      id: `needs-${id}`,
      name: `Needs ${id}`,
      cwd: '/fixture',
      created: '2026-10-20T08:00:00Z',
      modified: '2026-10-20T08:30:00Z',
      lastCompletedAt: '2026-10-20T08:30:00Z',
      streaming: false,
      needsAttention: true,
      attentionReason: 'failure',
      unread: true,
    })),
  ]
  let failLabels = false
  let releaseLabels
  let signalLabelRequest
  const labelRequestStarted = new Promise((resolve) => {
    signalLabelRequest = resolve
  })
  let holdLabels = false
  let labelRequests = 0
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/sessions') return route.fulfill({ json: { sessions } })
    if (url.pathname === '/api/session-labels') {
      labelRequests++
      if (holdLabels)
        await new Promise((resolve) => {
          releaseLabels = resolve
          signalLabelRequest()
        })
      return route.fulfill(
        failLabels
          ? { status: 503, json: { error: 'Fixture unavailable' } }
          : {
              json: {
                labels: [
                  {
                    sessionId: 'label-result',
                    entryId: 'entry',
                    label: `${url.searchParams.get('query')} label`,
                    sessionModified: '2026-10-20T00:00:00Z',
                    nodeTimestamp: '2026-10-20T00:00:00Z',
                    active: true,
                  },
                ],
              },
            },
      )
    }
    return route.fulfill({ json: {} })
  })
  const url = `http://127.0.0.1:${server.httpServer.address().port}/__shell`
  // 规格 v2 第 2.1 节：没有标题栏了，一级入口在 268 侧栏（新任务 / 搜索 / 全部代理 / 功能 / 设置），
  // 当前入口点亮，入口旁标出当前快捷键；⌘B 或按钮收起为 60px 窄轨，焦点留在收起 / 展开键上；
  // 任何宽度下默认展开占位，1440 / 1080 / 960 都不横向溢出；Windows 自绘标题栏时主区顶栏右侧让出 144px。
  await page.goto(url)
  const sidebar = page.locator('[data-shell-sidebar]')
  await sidebar.waitFor()
  const nav = sidebar.getByRole('navigation', { name: '会话与导航', exact: true })
  const entryIds = () =>
    sidebar
      .locator('[data-sidebar-entry]')
      .evaluateAll((entries) => entries.map((entry) => entry.getAttribute('data-sidebar-entry')))
  assert.deepEqual(await entryIds(), ['newTask', 'search', 'board', 'features', 'settings'])
  for (const [id, name] of [
    ['newTask', /^新任务/],
    ['search', /^搜索/],
    ['board', /^全部代理/],
    ['features', /^功能/],
    ['settings', /^设置/],
  ])
    assert.equal(await nav.getByRole('button', { name }).getAttribute('data-sidebar-entry'), id)
  const current = () =>
    sidebar
      .locator('[data-sidebar-entry][aria-current="page"]')
      .evaluateAll((entries) => entries.map((entry) => entry.getAttribute('data-sidebar-entry')))
  assert.deepEqual(await current(), ['board'])
  // 入口旁的按键提示与 aria-keyshortcuts 都跟随当前绑定（快捷键 v2）：全部代理 ⌘1、功能 ⌘2。
  assert.match(await sidebar.locator('[data-sidebar-entry="board"] kbd').innerText(), /1$/)
  assert.match(await sidebar.locator('[data-sidebar-entry="features"] kbd').innerText(), /2$/)
  assert.match(
    await sidebar.locator('[data-sidebar-entry="features"]').getAttribute('aria-keyshortcuts'),
    /\+2$/,
  )
  await nav.getByRole('button', { name: /^功能/ }).click()
  assert.equal(await page.getByTestId('opened').textContent(), 'entry:features')
  assert.deepEqual(await current(), ['features'])
  // 键盘：Tab 在入口之间移动，Enter 选中。
  await sidebar.locator('[data-sidebar-entry="search"]').focus()
  await page.keyboard.press('Tab')
  assert.equal(
    await page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-entry')),
    'board',
  )
  await page.keyboard.press('Enter')
  assert.deepEqual(await current(), ['board'])
  await nav.getByRole('button', { name: /^新任务/ }).click()
  assert.equal(await page.getByTestId('new-tasks').textContent(), '1')
  assert.equal(await page.getByTestId('groups-slot').count(), 1)
  // 分组区单独滚动；品牌行与入口、底部设置固定。浅色下侧栏与主区之间没有分隔线。
  const shellMetrics = () =>
    page.evaluate(() => {
      const bar = document.querySelector('[data-shell-sidebar]')
      const header = document.querySelector('main header')
      const style = getComputedStyle(bar)
      return {
        mode: bar.dataset.shellSidebar,
        width: Math.round(bar.getBoundingClientRect().width),
        border: style.borderRightWidth,
        scroll: getComputedStyle(bar.querySelector('[data-sidebar-scroll]') || bar).overflowY,
        pageOverflow: document.documentElement.scrollWidth > window.innerWidth + 1,
        headerRightmost: Math.max(
          0,
          ...[...header.querySelectorAll('button, input')].map(
            (node) => node.getBoundingClientRect().right,
          ),
        ),
      }
    })
  for (const width of [1440, 1080, 960]) {
    await page.setViewportSize({ width, height: 800 })
    await page.waitForFunction((expected) => window.innerWidth === expected, width)
    const metrics = await shellMetrics()
    assert.equal(metrics.mode, 'expanded', `the sidebar stays expanded at ${width}px`)
    assert.equal(metrics.width, 268, `sidebar width at ${width}px`)
    assert.equal(metrics.border, '0px', `no divider in light mode at ${width}px`)
    assert.equal(metrics.scroll, 'auto', `the group area scrolls on its own at ${width}px`)
    assert.equal(metrics.pageOverflow, false, `no overflow at ${width}px`)
  }
  // 收起：按钮收成 60px 窄轨（四个入口 + 计数插槽 + 设置），焦点落在展开键上；再展开焦点回到收起键。
  await page.setViewportSize({ width: 1440, height: 800 })
  await nav.getByRole('button', { name: '收起侧栏', exact: true }).click()
  await page.waitForFunction(
    () => document.querySelector('[data-shell-sidebar]')?.dataset.shellSidebar === 'rail',
  )
  await page.waitForFunction(
    () => document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect().width === 60,
  )
  assert.equal(
    await page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-toggle')),
    'expand',
  )
  assert.equal(await page.getByTestId('rail-counts-slot').count(), 1)
  assert.equal(await page.getByTestId('groups-slot').count(), 0)
  assert.deepEqual(await entryIds(), ['newTask', 'search', 'board', 'features', 'settings'])
  assert.equal(
    await sidebar.locator('[data-sidebar-entry="board"]').getAttribute('aria-label'),
    '全部代理',
  )
  await page.keyboard.press('Enter')
  await page.waitForFunction(
    () => document.querySelector('[data-shell-sidebar]')?.dataset.shellSidebar === 'expanded',
  )
  assert.equal(
    await page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-toggle')),
    'collapse',
  )
  // 收起偏好写进 pisper-sidebar（page-state-storage），重新载入后保持。
  await nav.getByRole('button', { name: '收起侧栏', exact: true }).click()
  await page.waitForFunction(() =>
    localStorage.getItem('pisper-sidebar')?.includes('"collapsed":true'),
  )
  await page.reload()
  await page.waitForFunction(
    () => document.querySelector('[data-shell-sidebar]')?.dataset.shellSidebar === 'rail',
  )
  await sidebar.getByRole('button', { name: '展开侧栏', exact: true }).click()
  await page.waitForFunction(() =>
    localStorage.getItem('pisper-sidebar')?.includes('"collapsed":false'),
  )
  // Windows 自绘标题栏：主区顶栏右侧 144px 让给窗口按钮，最小窗口 1080 时也不越界；
  // macOS / Linux 只留 12px（页头自己的右内边距）。
  await page.goto(`${url}?custom=1`)
  await sidebar.waitFor()
  for (const width of [1080, 1440]) {
    await page.setViewportSize({ width, height: 800 })
    await page.waitForFunction((expected) => window.innerWidth === expected, width)
    const metrics = await shellMetrics()
    assert.ok(
      metrics.headerRightmost <= width - 144 + 0.5,
      `custom title bar keeps the 144px window-control corner free at ${width}px: ${metrics.headerRightmost}`,
    )
    assert.equal(metrics.pageOverflow, false)
  }
  assert.equal(
    await page.locator('[data-shell-sidebar] [data-window-drag-region]').count(),
    1,
    'the sidebar brand row is a window drag region',
  )
  await page.setViewportSize({ width: 1200, height: 800 })
  await page.goto(url)
  await sidebar.waitFor()
  assert.ok(
    (await shellMetrics()).headerRightmost > 1200 - 144,
    'no reserve without custom titlebar',
  )
  await nav.getByRole('button', { name: /^搜索/ }).click()
  await page.getByRole('combobox').waitFor()
  // 主题键已从页头去掉，⌘K 里有“切换主题”。
  assert.equal(await page.locator('main header [aria-label*="主题"]').count(), 0)
  await page.getByRole('combobox').fill('主题')
  await page.getByRole('option', { name: /切换主题/ }).click()
  assert.equal(await page.getByTestId('opened').textContent(), 'theme')
  await page.getByRole('combobox').waitFor({ state: 'detached' })
  // 更多页搜索框把关键词带进命令面板（同一份数据源）。
  await page.getByTestId('launch-query').click()
  await page.getByRole('combobox').waitFor()
  assert.equal(await page.getByRole('combobox').inputValue(), 'Alpha')
  await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).waitFor()
  await page.keyboard.press('Escape')
  await page.getByRole('combobox').waitFor({ state: 'detached' })
  console.log(
    'PASS replica shell sidebar: five entries in order with the current one lit and its shortcut shown, keyboard selection, new task, 268px at 1440/1080/960 without overflow or divider, collapse to the 60px rail and back keeping focus on the toggle, persisted in pisper-sidebar, 144px custom title bar corner on the page header, search opens the palette with a 切换主题 command, More-search keyword hand-off',
  )

  // 手机底部标签栏：会话 · 看板 · [新任务 52×40] · 更多 · 设置；看板徽标在任何选中标签下都在；
  // 让出底部安全区；软键盘弹起时整栏隐藏；会话页（hidden）不渲染。
  const phonePage = await browser.newPage({
    viewport: { width: 390, height: 844 },
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    hasTouch: true,
    isMobile: true,
    deviceScaleFactor: 2,
  })
  await phonePage.clock.setFixedTime(new Date('2026-10-20T09:00:00Z'))
  phonePage.setDefaultTimeout(6000)
  phonePage.on('pageerror', (error) => errors.push(`phone: ${error.message}`))
  await phonePage.route('**/api/**', (route) => {
    const pathname = new URL(route.request().url()).pathname
    if (pathname === '/api/sessions') return route.fulfill({ json: { sessions: phoneSessions } })
    if (/\/api\/sessions\/[^/]+\/live$/.test(pathname))
      return route.fulfill({ json: { streaming: false, approvals: [] } })
    return route.fulfill({ json: {} })
  })
  await phonePage.addInitScript(() => {
    document.addEventListener('DOMContentLoaded', () =>
      document.documentElement.style.setProperty('--pisper-safe-area-bottom', '34px'),
    )
  })
  await phonePage.goto(`${url}?phone=1`)
  const tabBar = phonePage.getByTestId('mobile-tab-bar')
  await tabBar.waitFor()
  const boardTab = tabBar.locator('[data-shell-tab="board"]')
  await phonePage.waitForFunction(
    () =>
      document.querySelector('[data-shell-tab="board"]')?.getAttribute('aria-label') ===
      '看板, 2 项需要你',
  )
  const bar = await tabBar.evaluate((nav) => ({
    order: [...nav.children].map(
      (child) =>
        child.getAttribute('data-shell-tab') || (child.querySelector('button') ? 'new' : ''),
    ),
    newTask: (() => {
      const button = nav.querySelector(':scope > div > button')
      const box = button.getBoundingClientRect()
      return { name: button.getAttribute('aria-label'), width: box.width, height: box.height }
    })(),
    heights: [...nav.querySelectorAll('[data-shell-tab]')].map(
      (tab) => tab.getBoundingClientRect().height,
    ),
    paddingBottom: getComputedStyle(nav).paddingBottom,
    badge: nav.querySelector('[data-shell-tab="board"] [aria-hidden="true"].rounded-full')
      ?.textContent,
  }))
  assert.deepEqual(bar.order, ['session', 'board', 'new', 'more', 'settings'])
  assert.deepEqual(bar.newTask, { name: '新任务', width: 52, height: 40 })
  assert.ok(
    bar.heights.every((height) => height === 56),
    JSON.stringify(bar.heights),
  )
  assert.equal(bar.paddingBottom, '34px')
  assert.equal(bar.badge, '2')
  for (const tab of ['session', 'more', 'settings', null]) {
    await phonePage.evaluate((tab) => window.shellControls.setActive(tab), tab)
    assert.equal(
      await boardTab.getAttribute('aria-label'),
      '看板, 2 项需要你',
      `the board badge stays while ${tab} is active`,
    )
  }
  await tabBar.getByRole('button', { name: '更多', exact: true }).click()
  assert.equal(await phonePage.getByTestId('opened').textContent(), 'tab:more')
  assert.equal(await tabBar.locator('[data-shell-tab="more"]').getAttribute('aria-current'), 'page')
  await tabBar.getByRole('button', { name: '新任务', exact: true }).click()
  assert.equal(await phonePage.getByTestId('new-tasks').textContent(), '1')
  await phonePage.evaluate(() => {
    document.querySelector('.app-shell').dataset.mobileKeyboard = 'open'
  })
  assert.equal(await tabBar.isVisible(), false, 'the tab bar yields to the software keyboard')
  await phonePage.evaluate(() => {
    delete document.querySelector('.app-shell').dataset.mobileKeyboard
  })
  assert.equal(await tabBar.isVisible(), true)
  await phonePage.evaluate(() => window.shellControls.setHidden(true))
  await tabBar.waitFor({ state: 'detached' })
  await phonePage.screenshot({ path: join(output, 'tab-bar-hidden.png') })
  await phonePage.evaluate(() => window.shellControls.setHidden(false))
  await tabBar.waitFor()
  await phonePage.screenshot({ path: join(output, 'tab-bar.png') })
  await phonePage.close()
  console.log(
    'PASS replica shell tab bar: order with the 52x40 new task button, 56px tabs, 34px safe area, board badge on every tab, keyboard hides it, hidden renders nothing',
  )

  await page.setViewportSize({ width: 1200, height: 800 })
  await page.getByTestId('launch').click()
  const input = page.getByRole('combobox')
  await input.waitFor()
  const dialog = page.getByRole('dialog')
  await dialog.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished)),
  )
  assert.ok(Math.abs((await dialog.boundingBox()).width - 520) < 1)
  await input.dispatchEvent('compositionstart')
  await input.dispatchEvent('keydown', { key: 'Enter', isComposing: true })
  await input.dispatchEvent('keydown', { key: 'Escape', isComposing: true })
  assert.equal(
    await dialog.count(),
    1,
    'IME confirmation and cancellation must not activate or dismiss',
  )
  await input.dispatchEvent('compositionend')
  await input.press('Tab')
  const activeGroup = await input.evaluate((element) =>
    document
      .getElementById(element.getAttribute('aria-activedescendant'))
      .closest('[role=group]')
      .getAttribute('aria-label'),
  )
  assert.match(activeGroup, /页面|Pages/)
  failLabels = true
  await input.fill('Alpha')
  const retry = page
    .getByRole('option')
    .filter({ hasText: /无法搜索会话标记|Could not search conversation labels/ })
  await retry.waitFor()
  assert.equal(await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).count(), 1)
  failLabels = false
  await retry.click()
  await page.getByRole('option').filter({ hasText: 'Alpha label' }).waitFor()
  assert.ok(labelRequests >= 2)
  await input.fill('Failure')
  await page.getByRole('option').filter({ hasText: 'Failure conversation' }).click()
  await page.getByRole('alert').waitFor()
  assert.equal(await dialog.count(), 1, 'failed navigation retains the result and retry path')
  await input.fill('Alpha')
  await page.mouse.move(10, 10)
  await page.getByRole('option').filter({ hasText: 'Alpha label' }).waitFor()
  await input.fill('Alpha ')
  await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).hover()
  const before = await page.getByRole('option').allTextContents()
  holdLabels = true
  await page.evaluate(() => {
    window.dispatchEvent(new Event('focus'))
  })
  // 改查询后重新请求，指针进入已有会话结果时冻结快照。
  await input.fill('Alph')
  await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).hover()
  await page.waitForFunction(
    () => document.querySelector('[role=listbox]')?.getAttribute('aria-busy') === 'true',
  )
  let requestTimer
  try {
    await Promise.race([
      labelRequestStarted,
      new Promise((_, reject) => {
        requestTimer = setTimeout(
          () => reject(new Error('Pending label request was not observed')),
          6000,
        )
      }),
    ])
  } finally {
    clearTimeout(requestTimer)
  }
  releaseLabels()
  holdLabels = false
  await page.waitForFunction(
    () => document.querySelector('[role=listbox]')?.getAttribute('aria-busy') === 'false',
  )
  assert.equal(
    await page.getByRole('option').filter({ hasText: 'Alph label' }).count(),
    0,
    'late results cannot move the hovered target',
  )
  await page.mouse.move(10, 10)
  await page.getByRole('option').filter({ hasText: 'Alph label' }).waitFor()
  assert.ok(before.length)
  const desktopMetrics = await dialog.evaluate((element) => {
    const style = getComputedStyle(element)
    const rect = element.getBoundingClientRect()
    return {
      width: rect.width,
      height: rect.height,
      radius: style.borderRadius,
      fontFamily: style.fontFamily,
      fontSize: style.fontSize,
      rootFontSize: getComputedStyle(document.documentElement).fontSize,
      devicePixelRatio,
    }
  })
  await page.screenshot({ path: join(output, 'search-light.png') })
  await page.evaluate(() => {
    document.documentElement.dataset.theme = 'dark'
    document.documentElement.classList.add('dark')
  })
  await page.screenshot({ path: join(output, 'search-dark.png') })
  await page.setViewportSize({ width: 390, height: 420 })
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await dialog.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished)),
  )
  const narrow = await dialog.boundingBox()

  assert.ok(narrow.x >= 0 && narrow.x + narrow.width <= 390, JSON.stringify(narrow))
  await page.screenshot({ path: join(output, 'search-narrow.png') })
  assert.deepEqual(errors, [])
  await writeFile(
    join(output, 'metrics.json'),
    JSON.stringify(
      {
        browser: await browser.version(),
        locale: 'zh-CN',
        timezone: 'Asia/Shanghai',
        fixtureTime: '2026-10-20T09:00:00Z',
        desktop: { viewport: { width: 1200, height: 800 }, ...desktopMetrics },
        narrow: { viewport: { width: 390, height: 420 }, dialog: narrow },
        referenceScreenshotCompared: false,
      },
      null,
      2,
    ),
  )
  console.log(
    `PASS replica shell palette: IME, grouped Tab, errors/retry, pointer stability, narrow layout; snapshots ${output}`,
  )
} finally {
  await browser?.close()
  await server.close()
}
