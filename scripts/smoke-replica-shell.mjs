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
  // 手机顶栏 ☰ 旁的需要你计数：两个失败的会话落在“需要你”（不依赖 /live 的审批详情）。
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
  // 功能页模板（A6，插件 / 技能 / MCP 起）：主区顶栏 52 是没有控件的拖拽区，“‹ 功能”、标题与工具行都在它
  // 下方，所以自绘标题栏时右上角 144px 天然空着（1080 最小窗口也一样）；内容栏不超过 760（含内边距 808）。
  await page.goto(`${url}?custom=1&feature=1`)
  await sidebar.waitFor()
  for (const width of [1080, 1440]) {
    await page.setViewportSize({ width, height: 800 })
    await page.waitForFunction((expected) => window.innerWidth === expected, width)
    const template = await page.evaluate(() => {
      const main = document.querySelector('main')
      const top = main.getBoundingClientRect().top
      const drag = main.querySelector('[data-page-template="feature"] > [data-window-drag-region]')
      const controls = [...main.querySelectorAll('button, input, a[href], [role="switch"]')].filter(
        (node) => node.getClientRects().length > 0,
      )
      const boxes = controls.map((node) => node.getBoundingClientRect())
      return {
        drag: drag ? Math.round(drag.getBoundingClientRect().height) : 0,
        dragControls: drag?.querySelectorAll('button, input, a, [role="switch"]').length ?? -1,
        topmost: Math.min(...boxes.map((box) => box.top - top)),
        cornerHits: boxes.filter((box) => box.right > innerWidth - 144 && box.top - top < 52)
          .length,
        column: Math.round(
          main.querySelector('[data-feature-column]').getBoundingClientRect().width,
        ),
        heading: main.querySelector('h1')?.textContent,
        headingBelowBack:
          main.querySelector('h1').getBoundingClientRect().top >
          main.querySelector('[data-back-to-more]').getBoundingClientRect().bottom - 1,
        overflow: document.documentElement.scrollWidth > innerWidth + 1,
      }
    })
    assert.deepEqual(
      {
        drag: template.drag,
        dragControls: template.dragControls,
        cornerHits: template.cornerHits,
        heading: template.heading,
        headingBelowBack: template.headingBelowBack,
        overflow: template.overflow,
      },
      {
        drag: 52,
        dragControls: 0,
        cornerHits: 0,
        heading: '插件',
        headingBelowBack: true,
        overflow: false,
      },
      `feature template at ${width}px: ${JSON.stringify(template)}`,
    )
    assert.ok(template.topmost >= 52, `controls start below the drag bar: ${template.topmost}`)
    assert.ok(template.column <= 808, `760 content column (+48 padding): ${template.column}`)
  }
  // “‹ 功能”的读屏名称是“返回功能”，点它交给调用方（应用壳回到功能索引）；“/”认识的搜索框 id 不变。
  const featureBack = page.locator('[data-back-to-more]')
  assert.equal(await featureBack.getAttribute('aria-label'), '返回功能')
  assert.equal((await featureBack.innerText()).trim(), '功能')
  await featureBack.click()
  assert.equal(await page.getByTestId('opened').textContent(), 'back')
  assert.equal(await page.locator('#feature-page-search').count(), 1)
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
  // 1x 截图逐像素（规格 9.2）：运行中的墨色粗弧对比度明显高于排队虚线圈与完成勾，排队与完成形状不同。
  // 浅色与深色各一次；底色取字形截图左上角（圆与勾都碰不到的角落）。
  const glyphPixels = async (kind) => {
    const png = await page.locator(`[data-legend="${kind}"]`).screenshot({ animations: 'disabled' })
    return page.evaluate(async (data) => {
      const image = new Image()
      image.src = `data:image/png;base64,${data}`
      await image.decode()
      const canvas = document.createElement('canvas')
      canvas.width = image.width
      canvas.height = image.height
      const context = canvas.getContext('2d')
      context.drawImage(image, 0, 0)
      const { data: pixels } = context.getImageData(0, 0, image.width, image.height)
      const lum = (index) =>
        0.2126 * pixels[index] + 0.7152 * pixels[index + 1] + 0.0722 * pixels[index + 2]
      const background = lum(0)
      let maxDiff = 0
      const mask = []
      for (let index = 0; index < pixels.length; index += 4) {
        const diff = Math.abs(lum(index) - background)
        maxDiff = Math.max(maxDiff, diff)
        mask.push(diff > 40 ? 1 : 0)
      }
      return { width: image.width, maxDiff: Math.round(maxDiff), mask }
    }, png.toString('base64'))
  }
  const overlap = (a, b) => {
    let both = 0
    let either = 0
    a.forEach((value, index) => {
      if (value && b[index]) both += 1
      if (value || b[index]) either += 1
    })
    return either ? both / either : 1
  }
  const originalTheme = await page.evaluate(() => document.documentElement.dataset.theme ?? null)
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.documentElement.dataset.theme = value
    }, theme)
    const [run, queued, done] = await Promise.all(['run', 'queued', 'done'].map(glyphPixels))
    assert.equal(run.width, 16, `${theme}: glyphs are 16px at 1x`)
    assert.ok(
      run.maxDiff >= Math.max(queued.maxDiff, done.maxDiff) + 40,
      `${theme}: the ink running arc stands out (${run.maxDiff} vs ${queued.maxDiff} / ${done.maxDiff})`,
    )
    for (const [name, other] of [
      ['queued', queued],
      ['done', done],
    ])
      assert.ok(
        overlap(run.mask, other.mask) < 0.5,
        `${theme}: running and ${name} differ in shape`,
      )
    assert.ok(overlap(queued.mask, done.mask) < 0.5, `${theme}: queued and done differ in shape`)
  }
  await page.evaluate((value) => {
    if (value === null) delete document.documentElement.dataset.theme
    else document.documentElement.dataset.theme = value
  }, originalTheme)
  console.log(
    'PASS replica shell sidebar: five entries in order with the current one lit and its shortcut shown, keyboard selection, new task, 268px at 1440/1080/960 without overflow or divider, collapse to the 60px rail and back keeping focus on the toggle, persisted in pisper-sidebar, 144px custom title bar corner on the page header and on the feature-page template (52 drag-only bar, 760 column, ‹ 功能 above the title), 1x running arc / queued ring / done tick distinguishable pixel by pixel in light and dark, search opens the palette with a 切换主题 command, More-search keyword hand-off',
  )

  // 手机外壳（A4）：顶栏 ☰ + 抽屉取代底部标签栏。
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
  // 手机外壳（规格 v2 第 7.1 节）：没有底部标签栏；顶栏 56 = ☰（带需要你计数）· 页名 · 搜索 · ✎，☰ 打开抽屉。
  const topBar = phonePage.locator('[data-phone-topbar]')
  await topBar.waitFor()
  assert.equal(
    await phonePage.locator('[data-testid="mobile-tab-bar"], [data-shell-tab]').count(),
    0,
  )
  const menu = topBar.locator('[data-phone-menu]')
  await phonePage.waitForFunction(
    () =>
      document.querySelector('[data-phone-menu]')?.getAttribute('aria-label') ===
      '打开导航，2 个需要你',
  )
  const bar = await topBar.evaluate((header) => {
    const box = (element) => {
      const rect = element.getBoundingClientRect()
      return { width: Math.round(rect.width), height: Math.round(rect.height) }
    }
    const title = header.querySelector('[data-phone-title]').getBoundingClientRect()
    return {
      height: Math.round(header.getBoundingClientRect().height),
      menu: box(header.querySelector('[data-phone-menu]')),
      search: box(header.querySelector('[data-phone-search]')),
      newTask: box(header.querySelector('[data-phone-new-task]')),
      badge: header.querySelector('[data-phone-attention]')?.textContent,
      titleCenter: Math.round(title.left + title.width / 2),
      width: Math.round(header.getBoundingClientRect().width),
    }
  })
  assert.equal(bar.height, 56)
  assert.ok(bar.menu.height >= 44 && bar.menu.width >= 44, JSON.stringify(bar.menu))
  assert.deepEqual(bar.search, { width: 44, height: 44 })
  assert.deepEqual(bar.newTask, { width: 44, height: 44 })
  assert.equal(bar.badge, '2')
  assert.ok(Math.abs(bar.titleCenter - bar.width / 2) <= 2, JSON.stringify(bar))
  await topBar.getByRole('button', { name: '新任务', exact: true }).click()
  assert.equal(await phonePage.getByTestId('new-tasks').textContent(), '1')
  // 抽屉：min(320, 85vw)，搜索 · ✎ · 全部代理 · 功能 · 会话分组 · 设置；入口 44px；Esc 关闭后焦点回到 ☰。
  await menu.click()
  const drawer = phonePage.locator('[data-phone-drawer]')
  await drawer.waitFor()
  assert.equal(await menu.getAttribute('aria-expanded'), 'true')
  const layout = await drawer.evaluate((panel) => ({
    width: Math.round(panel.getBoundingClientRect().width),
    entries: [...panel.querySelectorAll('[data-drawer-entry]')].map((entry) => [
      entry.getAttribute('data-drawer-entry'),
      Math.round(entry.getBoundingClientRect().height),
    ]),
    groups: Boolean(panel.querySelector('[data-testid="drawer-groups-slot"]')),
    current: panel.querySelector('[aria-current="page"]')?.getAttribute('data-drawer-entry'),
  }))
  assert.equal(layout.width, 320)
  assert.deepEqual(layout.entries, [
    ['board', 44],
    ['features', 44],
    ['settings', 44],
  ])
  assert.equal(layout.groups, true)
  assert.equal(layout.current, 'board')
  await phonePage.screenshot({ path: join(output, 'phone-drawer.png') })
  await phonePage.keyboard.press('Escape')
  await drawer.waitFor({ state: 'detached' })
  await phonePage.waitForFunction(() => document.activeElement?.hasAttribute('data-phone-menu'))
  // 入口先关抽屉再执行。
  await menu.click()
  await drawer.waitFor()
  await drawer.locator('[data-drawer-entry="features"]').click()
  await drawer.waitFor({ state: 'detached' })
  assert.equal(await phonePage.getByTestId('opened').textContent(), 'entry:features')
  await menu.click()
  await drawer.locator('[data-drawer-new-task]').click()
  await drawer.waitFor({ state: 'detached' })
  assert.equal(await phonePage.getByTestId('new-tasks').textContent(), '2')
  await phonePage.screenshot({ path: join(output, 'phone-top-bar.png') })
  await phonePage.close()
  console.log(
    'PASS replica shell phone top bar and drawer: no tab bar, 56px top bar with ☰ + amber count, centred title, 44px search / new task; drawer 320 wide with 44px entries, current entry, Esc returns focus to ☰, entries close the drawer first',
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
