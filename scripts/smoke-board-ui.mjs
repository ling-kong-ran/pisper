// 全部代理（原看板）与新外壳冒烟：真实生产构建 + 隔离的临时 Runtime；会话列表、/live、审批与重试
// 只在浏览器边界用固定数据提供（scripts/fixtures/board-fixture.mjs），不连接模型、不写真实数据。
// 规格 v2 第 3 节（A3 取代泳道）：1440×900 / 1080×680、浅色 / 深色、模拟 Windows 自绘标题栏（右上 144px
// 不被遮挡、无硬裁切、无横向滚动）、按状态分组的列表（最大 880 居中）与按项目分组、行内批准与重试、
// critical 只能打开处理（进会话）、侧栏的状态 / 项目筛选与计数（规格 2.3）、⌘1 / ⌘2、⌘N 与“新任务”打开派发对话框、逐个处理
// （其余分组 50%、只在需要你之间走）、行焦点环、未聚焦时动作键无效、项目 ⋯（重命名、恢复、管理项目）、
// 今天完成折叠、筛选说明与清除、空状态、手机底部标签栏，以及窄化的桌面窗口不进入手机外壳。
// 第 2 期：固定数据默认模拟 board.view / board.events（事件流 + view=board，带 card 摘要），
// 另有一段把能力里的 board.* 去掉，验证旧桌面 Runtime 退回第 1 期的 /api/sessions + /live 路径；
// 再用被测的真实 Runtime 提供 /api/board/events（长连接），核对每个标签页只有 1 条连接、快照与增量、
// 页面隐藏时断开、回到可见后带 after 重连。
// 第 3 期会话页：隔离 Runtime 里写入一条真实会话（转录、/live 来自真实 Runtime），看板与侧栏分组仍由
// 固定数据提供；覆盖侧栏需要你行的快捷处理（普通批准一步允许、critical / 失败只能打开）、⌘J 聚焦侧栏列表、
// ⌥A 聚焦等待最久的需要你行、⌥↑/⌥↓ 按侧栏顺序切换、⌘B 收起为窄轨与窄轨计数、⌘I 检查器、
// ⌘1 回到看板并把焦点还给来源卡片（快捷键 v2：⇧⌘B 归会话树）、运行中 Esc 只停止不返回、1080×680 + 自绘标题栏的检查器覆盖层，
// 以及手机 390 的会话页仍可用。
// 第 4 期：快捷键改绑（⌘J / ⌘I / 插话改绑后立即生效、旧键失效）与 ⌘/ 速查（快捷键 v2：⌘1 全部代理、
// ⌘2 功能，⌘[ 已退役）。泳道条、⌘J 代理面板与标题栏舰队计量已在 v2（A1）删除。
// 浏览器模拟不等于 Tauri / WebView2 或真机验证。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import {
  BOARD_SESSION_ID,
  BOARD_SESSION_TITLE,
  boardFixture,
  desktopBridgeScript,
  installBoardFixture,
  plainDesktopBridgeScript,
  QUESTION_ID,
  QUESTION_SESSION_ID,
  questionFixtureSession,
  writeBoardSessionFixture,
} from './fixtures/board-fixture.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = await mkdtemp(join(tmpdir(), 'pisper-board-ui-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
const fixtureHome = join(output, 'home')
await mkdir(workspace)
await mkdir(fixtureHome)
// 所有 CLI 扫描限于空的隔离 home；看板偏好经 Runtime 同步，也只写进这个临时目录。
process.env.HOME = fixtureHome
process.env.USERPROFILE = fixtureHome
process.env.CODEX_HOME = join(fixtureHome, '.codex')
process.env.CLAUDE_CONFIG_DIR = join(fixtureHome, '.claude')
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace
// 会话页用的真实会话：只写进本次的临时数据目录。
await writeBoardSessionFixture(join(dataDir, 'sessions'), workspace)
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
// 应用快捷键的 Mod 在 macOS 是 ⌘，其余平台是 Ctrl；浏览器按 navigator.platform 判断。
const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
const report = { status: 'running', checks: [], geometry: [], pageErrors: [] }
let browser
let failurePage

async function newContext(options, { bridge, mobileApp, sessions, protocol } = {}) {
  const context = await browser.newContext({ locale: 'zh-CN', ...options })
  await context.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  if (bridge) await context.addInitScript(bridge)
  if (mobileApp) await context.addInitScript('window.__PISPER_MOBILE_APP__ = true')
  const fixture = await installBoardFixture(context, { sessions, protocol })
  // 更新检查不是本冒烟的验收目标，固定结果避免依赖网络。
  await context.route('**/api/app-update*', (route) =>
    route.fulfill({ json: { state: 'current', checkedAt: Date.now() } }),
  )
  return { context, fixture }
}

async function openBoard(context, label) {
  const page = await context.newPage()
  failurePage = page
  page.on('pageerror', (error) => report.pageErrors.push(`${label}: ${error.message}`))
  await page.goto(base + '/#/board')
  await page.locator('[data-board-root]').waitFor({ timeout: 60000 })
  return page
}

// 审批详情来自 /live（错峰拉取）或 card：详情到达前普通批准没有“允许”、高风险看不到“高风险”说明，先等它就绪。
async function waitForApprovals(page) {
  await page
    .locator('[data-board-card="fx-critical"] [data-severity="critical"]')
    .waitFor({ timeout: 20000 })
  await page
    .locator('[data-board-card="fx-standard"]')
    .getByRole('button', { name: /^允许/ })
    .and(page.locator(':enabled'))
    .waitFor({ timeout: 20000 })
}

// 行所在的分区（按状态分组时是 attention / running / queued / done）。
function rowSection(page, id) {
  return page
    .locator(`[data-board-card="${id}"]`)
    .evaluate((row) =>
      row.closest('[data-overview-section]')?.getAttribute('data-overview-section'),
    )
}

async function settle(page) {
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await page.waitForTimeout(300)
}

// 几何自查：文档横向滚动、自绘标题栏右上 144px（窗口按钮专用区）是否被可交互元素占用、
// 标题栏与看板里的按钮是否被硬裁切（内容宽于自身且没有省略处理）。
async function geometry(page, label) {
  const result = await page.evaluate(() => {
    const custom = Boolean(window.pisperDesktop?.customTitlebar)
    const reservedLeft = window.innerWidth - 144
    const interactive = [
      ...document.querySelectorAll(
        'button, a[href], input, textarea, select, [role="button"], [role="tab"], [tabindex="0"]',
      ),
    ]
      .filter((element) => element.getClientRects().length > 0)
      .map((element) => ({ element, box: element.getBoundingClientRect() }))
      .filter(({ box }) => box.width > 0 && box.height > 0)
    const name = (element) =>
      (element.getAttribute('aria-label') || element.textContent || element.tagName)
        .trim()
        .slice(0, 40)
    const intrusions = custom
      ? interactive
          .filter(({ box }) => box.top < 48 && box.right > reservedLeft + 0.5)
          .filter(({ element }) => !element.closest('.desktop-window-controls'))
          .map(
            ({ element, box }) =>
              `${name(element)}@${Math.round(box.left)}-${Math.round(box.right)}`,
          )
      : []
    const clipped = interactive
      .filter(({ element }) => element.closest('[data-shell-sidebar], [data-board-root]'))
      .filter(({ element }) => element.tagName === 'BUTTON')
      .filter(({ element }) => element.scrollWidth > element.clientWidth + 1)
      .map(({ element }) => name(element))
    const cardOverflow = [...document.querySelectorAll('[data-board-card]')]
      .filter((card) => card.scrollWidth > card.clientWidth + 1)
      .map((card) => card.getAttribute('data-board-card'))
    return {
      width: window.innerWidth,
      horizontalScroll:
        document.documentElement.scrollWidth > window.innerWidth ||
        document.body.scrollWidth > window.innerWidth,
      intrusions,
      clipped,
      cardOverflow,
      windowControls: document.querySelectorAll('.desktop-window-controls button').length,
      sections: [...document.querySelectorAll('[data-overview-section]')].map((section) =>
        section.getAttribute('data-overview-section'),
      ),
      list:
        document.querySelector('[data-overview-list]')?.getBoundingClientRect().toJSON() ?? null,
      main:
        document.querySelector('[data-overview-scroller]')?.getBoundingClientRect().toJSON() ??
        null,
    }
  })
  report.geometry.push({ label, ...result })
  assert.equal(result.horizontalScroll, false, `${label}: document must not scroll horizontally`)
  assert.deepEqual(result.intrusions, [], `${label}: top-right 144px stays free for window buttons`)
  assert.deepEqual(result.clipped, [], `${label}: no hard-clipped buttons`)
  assert.deepEqual(result.cardOverflow, [], `${label}: card content never overflows its card`)
  return result
}

// 全部代理页侧栏状态筛选上的计数（规格 2.3：这一页侧栏是筛选，不列会话行），等到与期望一致。
async function waitForFilterCounts(page, expected) {
  await page.waitForFunction(
    (counts) =>
      Object.entries(counts).every(
        ([filter, count]) =>
          document.querySelector(
            `[data-shell-sidebar] [data-sidebar-filter="${filter}"] [data-sidebar-filter-count]`,
          )?.textContent === String(count),
      ),
    expected,
    { timeout: 15_000 },
  )
}

// 看板筛选偏好经 Runtime 的偏好镜像跨上下文同步：用过窄轨计数的筛选后清掉，不影响之后的上下文。
async function resetBoardView() {
  const response = await fetch(`${base}/api/local/browser-preferences`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      updates: { 'pisper-board-view': null },
      revisions: { 'pisper-board-view': Date.now() * 1000 + 999 },
    }),
  })
  assert.ok(response.ok, `reset the board view preference: ${response.status}`)
}

try {
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })

  // 1. 桌面尺寸 × 主题矩阵：每一档都注入自绘标题栏桥接，检查几何与分组列表结构。
  for (const [width, height] of [
    [1440, 900],
    [1080, 680],
  ]) {
    for (const colorScheme of ['light', 'dark']) {
      const label = `${width}x${height} ${colorScheme}`
      const { context } = await newContext(
        { viewport: { width, height }, colorScheme },
        { bridge: desktopBridgeScript },
      )
      const page = await openBoard(context, label)
      await waitForApprovals(page)
      await settle(page)
      await page.waitForFunction(
        (theme) => document.documentElement.dataset.theme === theme,
        colorScheme,
      )
      // 规格 v2：没有标题栏，任何宽度下侧栏都默认展开占位（268px）。
      assert.equal(await page.locator('[data-shell-sidebar="expanded"]').isVisible(), true)
      assert.equal(
        Math.round((await page.locator('[data-shell-sidebar]').boundingBox()).width),
        268,
      )
      assert.equal(await page.locator('[data-app-titlebar]').count(), 0)
      assert.equal(await page.locator('[data-custom-titlebar]').count(), 1)
      assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
      assert.equal(
        await page.locator('[data-board-root] h1').innerText(),
        '全部代理',
        `${label}: the page is called 全部代理`,
      )
      assert.match(
        await page.locator('[data-overview-summary]').innerText(),
        /^7 个代理 · 4 个项目 · 2 个在运行$/,
      )
      const result = await geometry(page, label)
      assert.equal(
        result.windowControls,
        3,
        `${label}: three window buttons in the top-right corner`,
      )
      // 泳道、启动台与任务输入框都退役了：这一页只有分组列表。
      assert.equal(await page.locator('[data-board-lane], [data-launch-console]').count(), 0)
      // 第 1 期 Runtime 不声明 sessionQueue：没有排队分组；需要你 → 运行中 → 今天完成。
      assert.deepEqual(result.sections, ['attention', 'running', 'done'])
      for (const [section, ids] of Object.entries({
        attention: ['fx-critical', 'fx-standard', 'fx-failed'],
        running: ['fx-running', 'fx-running-agents'],
        done: ['fx-done-unread', 'fx-done-pinned'],
      }))
        for (const id of ids)
          assert.equal(await rowSection(page, id), section, `${label}: ${id} sits in ${section}`)
      // 需要你按等待时长（最久的在前）。
      assert.deepEqual(
        await page
          .locator('[data-overview-section="attention"] [data-board-card]')
          .evaluateAll((rows) => rows.map((row) => row.getAttribute('data-board-card'))),
        ['fx-failed', 'fx-critical', 'fx-standard'],
      )
      // 居中的列表最大 880（规格 3.1）；没有组件时不出现右侧组件栏，也不留空位。
      assert.ok(
        result.list.width <= 880.5,
        `${label}: list is at most 880 wide (${result.list.width})`,
      )
      const leftGap = result.list.left - result.main.left
      const rightGap = result.main.right - result.list.right
      assert.ok(
        Math.abs(leftGap - rightGap) <= 24,
        `${label}: list is centred (${leftGap}/${rightGap})`,
      )
      assert.equal(await page.locator('[data-overview-widgets]').count(), 0)
      // 行高随内容 52–96（普通批准的完整命令最多 3 行时会更高，这里只看没有命令的行）。
      for (const id of ['fx-running', 'fx-done-pinned', 'fx-failed']) {
        const box = await page.locator(`[data-board-card="${id}"]`).boundingBox()
        assert.ok(box.height >= 52 && box.height <= 96, `${label}: ${id} row is ${box.height}px`)
      }
      await page.screenshot({ path: join(output, `board-${width}-${colorScheme}.png`) })
      await context.close()
    }
  }
  report.checks.push(
    '1440x900 and 1080x680, light and dark, simulated Windows custom title bar: the 268px sidebar stays expanded, no title bar, no horizontal scroll, top-right 144px free of interactive elements, no hard-clipped buttons; the page is 全部代理 with “7 个代理 · 4 个项目 · 2 个在运行”, a centred list ≤880 grouped 需要你 (longest wait first) → 运行中 → 今天完成, no lanes, no launch console, no widget column, rows 52–96 tall',
  )

  // 2. 1440 浅色上的交互：侧栏筛选与计数（规格 2.3）、逐个处理、critical 只能打开处理、普通批准、重试、快捷键、
  // 派发对话框、按项目分组与项目 ⋯、行 ⋯ 菜单。
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'interactions')
    await waitForApprovals(page)
    // 侧栏随页面变化（规格 2.3）：全部代理页的侧栏不列会话行（代理只在主区列一次），收成状态与项目两组筛选，
    // 各带计数（与主区同一份代理、按筛选前计）。需要你行的快捷处理与读屏名称在会话页一节（7a）与 smoke-sidebar 核对。
    const sidebar = page.locator('[data-shell-sidebar]')
    const filter = (id) => sidebar.locator(`[data-sidebar-filter="${id}"]`)
    await waitForFilterCounts(page, { all: 7, attention: 3, running: 2, done: 2 })
    assert.equal(await sidebar.locator('[data-sidebar-row]').count(), 0, 'no session rows here')
    assert.equal(await filter('queued').count(), 0, 'no queue filter without sessionQueue')
    const filterState = () =>
      page.evaluate(() =>
        [...document.querySelectorAll('[data-shell-sidebar] [data-sidebar-filter]')].map(
          (button) => [button.dataset.sidebarFilter, button.getAttribute('aria-pressed')],
        ),
      )
    const projectFilters = (await filterState()).filter(([id]) => id.startsWith('project:'))
    assert.ok(projectFilters.length >= 3, `one filter row per project ${projectFilters}`)
    const projectTotal = await sidebar
      .locator('[data-sidebar-filter^="project:"] [data-sidebar-filter-count]')
      .evaluateAll((nodes) => nodes.reduce((sum, node) => sum + Number(node.textContent), 0))
    assert.equal(projectTotal, 7, 'project counts add up to all agents')
    assert.equal(await filter('all').getAttribute('aria-pressed'), 'true')
    assert.equal(await filter('all').getAttribute('id'), 'board-filter')
    // 需要你计数为琥珀（规格 6.1 琥珀只表示需要你），运行中不是。
    const amber = await page.evaluate(() => {
      const probe = document.createElement('span')
      probe.style.color = 'var(--warning-strong)'
      document.body.append(probe)
      const color = getComputedStyle(probe).color
      probe.remove()
      const count = (id) =>
        getComputedStyle(
          document.querySelector(
            `[data-shell-sidebar] [data-sidebar-filter="${id}"] [data-sidebar-filter-count]`,
          ),
        ).color
      return { amber: color, attention: count('attention'), running: count('running') }
    })
    assert.equal(amber.attention, amber.amber)
    assert.notEqual(amber.running, amber.amber)
    // 点“运行中”：主区只剩运行中（与 pisper-board-view 同一份筛选），再点一次回到全部。
    await filter('running').click()
    assert.equal(await filter('running').getAttribute('aria-pressed'), 'true')
    assert.equal(await filter('all').getAttribute('aria-pressed'), 'false')
    await page.locator('[data-board-card="fx-standard"]').waitFor({ state: 'detached' })
    await page.locator('[data-board-card="fx-running"]').waitFor()
    await page.waitForFunction(() =>
      JSON.parse(localStorage.getItem('pisper-board-view') || '{}').state?.lanes?.includes(
        'running',
      ),
    )
    // 侧栏展开时筛选看得见，页内不再重复一行筛选说明。
    assert.equal(await page.locator('[data-overview-filter]').count(), 0)
    await filter('running').click()
    await page.locator('[data-board-card="fx-standard"]').waitFor()
    // 项目筛选：只看一个项目，计数不随状态筛选变；键盘 ↑ / ↓ 在筛选之间移动，空格切换。
    const [firstProject] = projectFilters[0]
    await filter('all').focus()
    await page.keyboard.press('ArrowDown')
    assert.equal(
      await page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-filter')),
      'attention',
    )
    await page.keyboard.press('End')
    await page.keyboard.press('Home')
    assert.equal(
      await page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-filter')),
      'all',
    )
    const projectButton = sidebar.locator(`[data-sidebar-filter="${firstProject}"]`)
    await projectButton.focus()
    await page.keyboard.press('Space')
    assert.equal(await projectButton.getAttribute('aria-pressed'), 'true')
    // 只筛了项目时“全部”不再按下（否则“全部 7”读起来像主区在显示全部 7 个）。
    assert.equal(await filter('all').getAttribute('aria-pressed'), 'false')
    const visible = await page
      .locator('[data-board-card]')
      .evaluateAll((rows) => rows.map((row) => row.getAttribute('data-board-card')))
    assert.ok(visible.length > 0 && visible.length < 7, `project filter narrows ${visible}`)
    await page.keyboard.press('Space')
    assert.equal(await projectButton.getAttribute('aria-pressed'), 'false')
    await page.locator('[data-board-card="fx-done-unread"]').waitFor()
    // “/”聚焦侧栏的第一个筛选（全部）。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press('/')
    await page.waitForFunction(() => document.activeElement?.id === 'board-filter')
    // 项目组 ⋯：添加项目、管理项目（沿用项目命令与管理对话框）。
    await sidebar.locator('[data-sidebar-projects-menu]').click()
    const projectMenu = page.getByRole('menu')
    await projectMenu.getByRole('menuitem', { name: '添加项目…' }).waitFor()
    await projectMenu.getByRole('menuitem', { name: '管理项目…' }).click()
    const manageFromSidebar = page.getByRole('dialog', { name: '管理项目' })
    await manageFromSidebar.waitFor()
    await page.keyboard.press('Escape')
    await manageFromSidebar.waitFor({ state: 'hidden' })
    await page.screenshot({ path: join(output, 'board-sidebar-filters.png') })
    report.checks.push(
      'All agents sidebar (spec 2.3): no session rows; status filters 全部 7 · 需要你 3 (amber) · 运行中 2 · 今天完成 2 (no 排队 without sessionQueue) and one row per project whose counts add up to 7; 运行中 narrows the list through pisper-board-view (aria-pressed, no duplicate in-page filter line while the sidebar is expanded) and a second click clears it; ↑ / ↓ / Home / End walk the filters, Space toggles a project filter; / focuses 全部; the 项目 ⋯ menu offers 添加项目 and opens 管理项目',
    )

    // 行内容（规格 3.3）：普通批准第 1 行状态“等你批准 · bash”、第 2 行直接是完整命令（不截断）与“拒绝 D · 允许 A”；
    // 高风险“高风险 · 改写远端分支”（琥珀文字）只有“打开处理”；失败一行等宽摘要与“日志 · 重试 R”；
    // 运行中当前活动与子代理数；今天完成的结果摘要与改动统计。一行最多一个石墨主操作。
    const row = (id) => page.locator(`[data-board-card="${id}"]`)
    assert.match(
      await row('fx-standard').locator('[data-row-status]').innerText(),
      /^等你批准 · bash$/,
    )
    const command = await row('fx-standard').locator('[data-command-text]').innerText()
    assert.match(command, /--reporter=verbose$/, 'the full command is shown, never truncated')
    // 命令只在空白处换行：每个片段是一个原子行内框，“--coverage.include”“-n1”不会在连字符后断成两段
    // 读起来像两个参数；没有一条视觉行以 “-” 结尾（片段本身比一行还宽时除外，fixture 里没有）。
    const hyphenBreaks = await page
      .locator('[data-overview] [data-command-text]')
      .evaluateAll((nodes) =>
        nodes.flatMap((pre) => {
          const width = pre.clientWidth
          return [...pre.querySelectorAll('[data-command-token]')]
            .filter((token) => token.scrollWidth <= width && token.getClientRects().length > 0)
            .filter((token) => token.getBoundingClientRect().height > 21)
            .map((token) => token.textContent)
        }),
      )
    assert.deepEqual(hyphenBreaks, [], 'command tokens never wrap inside themselves')
    assert.equal(
      await row('fx-standard').locator('[data-command-text]').textContent(),
      '$ ' + command.replace(/^\$\s*/, ''),
      'copied text stays the original command (no soft hyphens or joiners)',
    )
    const rowHeights = await page
      .locator('[data-board-card]')
      .evaluateAll((nodes) =>
        nodes.map((node) => [
          node.getAttribute('data-board-card'),
          node.getBoundingClientRect().height,
        ]),
      )
    for (const [id, height] of rowHeights)
      assert.ok(height >= 52 && height <= 96, `${id} row height ${height} within 52–96`)
    // 行 ⋯ 不占位：时间列贴着行的右内边距（12px），悬停时 ⋯ 盖在时间的位置上。
    const timeGap = await row('fx-done-unread').evaluate((node) => {
      const time = node.querySelector('[data-row-time]').getBoundingClientRect()
      return Math.round(node.getBoundingClientRect().right - time.right)
    })
    assert.ok(timeGap <= 13, `time sits at the row's right edge (${timeGap}px)`)
    await row('fx-standard').getByRole('button', { name: /^拒绝/ }).waitFor()
    const critical = row('fx-critical')
    assert.match(
      await critical.locator('[data-severity="critical"]').innerText(),
      /高风险 · 改写远端分支/,
    )
    const criticalColor = await critical.locator('[data-severity="critical"]').evaluate((node) => {
      const probe = document.createElement('span')
      probe.style.color = 'var(--warning-strong)'
      document.body.append(probe)
      const amber = getComputedStyle(probe).color
      probe.remove()
      return { color: getComputedStyle(node).color, amber }
    })
    assert.equal(criticalColor.color, criticalColor.amber, 'high risk is amber text, not red')
    assert.match(await critical.innerText(), /git push --force-with-lease origin fix\/ci/)
    await critical.getByRole('button', { name: '打开处理' }).waitFor()
    assert.equal(await critical.getByRole('button', { name: /^(允许|拒绝)/ }).count(), 0)
    // 高风险说明是命令的前缀：不随命令一起被选中复制。
    assert.equal(
      await critical
        .locator('[data-command-text] [data-severity="critical"]')
        .evaluate((node) => getComputedStyle(node.parentElement).userSelect),
      'none',
    )
    assert.match(
      await row('fx-failed').locator('[data-card-error]').innerText(),
      /3 failed, 41 passed/,
    )
    await row('fx-failed').getByRole('button', { name: '日志', exact: true }).waitFor()
    await row('fx-failed').getByRole('button', { name: /^重试/ }).waitFor()
    assert.match(
      await row('fx-running').locator('[data-card-activity]').innerText(),
      /bash · npm test/,
    )
    assert.match(
      await row('fx-running-agents').locator('[data-card-agents]').innerText(),
      /子代理 1\/2/,
    )
    assert.match(await row('fx-done-unread').locator('[data-card-preview]').innerText(), /LCP/)
    assert.match(await row('fx-done-unread').innerText(), /6 个文件\s*\+214\s*−97/)
    const primaries = await page.locator('[data-board-card]').evaluateAll((rows) =>
      rows.map((node) => ({
        id: node.getAttribute('data-board-card'),
        primary: [...node.querySelectorAll('button')].filter(
          (button) => button.getAttribute('data-variant') === 'default',
        ).length,
      })),
    )
    for (const entry of primaries)
      assert.ok(entry.primary <= 1, `${entry.id} has at most one primary action`)
    report.checks.push(
      'Rows: a standard approval shows “等你批准 · bash” on line 1 and the full untruncated command on line 2 (wrapped only at spaces, copied text unchanged) with 拒绝 / 允许; every row is 52–96 tall and the time column sits at the right edge; the critical row shows amber “高风险 · 改写远端分支” with the full command and only 打开处理; the failed row has a mono summary with 日志 / 重试; running rows show the current activity and 子代理 1/2; the done row shows its preview and “6 个文件 +214 −97”; no row has more than one primary action',
    )

    // T 进入逐个处理：其余分组降到 50%，底部提示条；J 只在需要你之间走；Esc 退出。
    await critical.focus()
    await page.keyboard.press('t')
    const triageBar = page.locator('[data-triage-bar]')
    await triageBar.waitFor()
    assert.match(await triageBar.innerText(), /逐个处理 1\/3/)
    assert.match(await triageBar.innerText(), /允许[\s\S]*拒绝[\s\S]*重试[\s\S]*打开[\s\S]*退出/)
    // A5 起 Runtime 声明 askUser：提示条带“1–4 回答”（没有这个能力的旧 Runtime 不写，见 overview 单测）。
    assert.match(await triageBar.innerText(), /1–4[\s\S]*回答/, 'the answer hint follows askUser')
    // 透明度有一次 --d-fast 的过渡：等它走完再读。
    const opacityOf = (id, value) =>
      page.waitForFunction(
        ([target, expected]) =>
          getComputedStyle(document.querySelector(`[data-board-card="${target}"]`)).opacity ===
          expected,
        [id, value],
      )
    await opacityOf('fx-running', '0.5')
    await opacityOf('fx-critical', '1')
    // 其余分组整体退后：分组标题与“全部历史会话”一起降到 50%，需要你的标题不变。
    await page.waitForFunction(
      () =>
        getComputedStyle(document.querySelector('[data-overview-heading="running"]')).opacity ===
          '0.5' &&
        getComputedStyle(document.querySelector('[data-overview-history]').parentElement)
          .opacity === '0.5' &&
        getComputedStyle(document.querySelector('[data-overview-heading="attention"]')).opacity ===
          '1',
    )
    for (let step = 0; step < 4; step += 1) {
      await page.keyboard.press('j')
      await page.waitForTimeout(80)
      assert.equal(
        await page.evaluate(() => document.activeElement?.getAttribute('data-lane')),
        'attention',
        'triage only walks the needs-you rows',
      )
    }
    await page.keyboard.press('Escape')
    await triageBar.waitFor({ state: 'hidden' })
    await opacityOf('fx-running', '1')
    report.checks.push(
      'T enters triage with the bottom bar “逐个处理 1/3 · A 允许 · D 拒绝 · R 重试 · ↵ 打开 · Esc 退出” (no answer hint without askUser), other groups (rows, their headings and the history link) drop to 50%, J only walks the needs-you rows, Esc exits',
    )

    // J 从空白处移动焦点：行的 2px 焦点环必须真的画出来（--focus 色），不能被 outline-none 吃掉。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press('j')
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-board-card'))
    const ring = await page.evaluate(() => {
      const style = getComputedStyle(document.activeElement)
      const probe = document.createElement('span')
      probe.style.color = 'var(--focus)'
      document.body.append(probe)
      const focus = getComputedStyle(probe).color
      probe.remove()
      return {
        style: style.outlineStyle,
        width: style.outlineWidth,
        color: style.outlineColor,
        focus,
      }
    })
    assert.notEqual(ring.style, 'none', 'the focused row shows an outline')
    assert.equal(ring.width, '2px')
    assert.equal(ring.color, ring.focus, 'the focus ring uses --focus')
    // 只有当前行在 Tab 序列里（roving tabindex）。
    assert.equal(await page.locator('[data-board-card][tabindex="0"]').count(), 1)
    // 焦点不在任何行上时，A / D / R 不能落到“第一行”上。
    await page.evaluate(() => document.activeElement?.blur())
    for (const key of ['r', 'a', 'd']) await page.keyboard.press(key)
    await page.waitForTimeout(500)
    assert.equal(fixture.posts.length, 0, 'action keys without a focused row send nothing')
    assert.equal(await page.getByRole('dialog').count(), 0)
    report.checks.push(
      'J shows a visible 2px --focus ring on the focused row and only one row is in the Tab order; A / D / R with no row focused send nothing and open nothing',
    )

    // critical：键盘 A、D 都什么也不做（没有确认层，也不批准或拒绝，与行上只有“打开处理”一致）；
    // ↵ 打开会话处理，⌘1 回来焦点还给这一行。
    await critical.focus()
    await page.keyboard.press('a')
    await page.keyboard.press('d')
    await page.waitForTimeout(400)
    assert.equal(
      fixture.posts.length,
      0,
      'keyboard A / D on a critical row must not approve or deny',
    )
    assert.equal(await page.getByRole('dialog').count(), 0, 'no confirmation layer on this page')
    assert.equal(await page.evaluate(() => location.hash), '#/board')
    await page.keyboard.press('Enter')
    await page.waitForFunction(() => location.hash === '#/board/fx-critical')
    await page.keyboard.press(`${mod}+Digit1`)
    await page.locator('[data-board-root]').waitFor()
    await page.waitForFunction(
      () => document.activeElement?.getAttribute('data-board-card') === 'fx-critical',
      null,
      { timeout: 10_000 },
    )
    assert.equal(fixture.posts.length, 0)
    report.checks.push(
      'Critical approval: keyboard A and D do nothing (no confirmation layer, no POST); ↵ opens the session to handle it and ⌘1 returns focus to the same row',
    )

    // standard：焦点在行上按 A，发出正确的 POST；行收起后进运行中，焦点落到下一条需要你。
    await waitForApprovals(page)
    await row('fx-standard').focus()
    const [approveResponse] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname ===
            '/api/sessions/fx-standard/approvals/approval-standard',
      ),
      page.keyboard.press('a'),
    ])
    assert.equal(approveResponse.request().postData(), JSON.stringify({ approved: true }))
    assert.deepEqual(fixture.posts, [
      {
        method: 'POST',
        path: '/api/sessions/fx-standard/approvals/approval-standard',
        body: { approved: true },
      },
    ])
    await page.locator('[data-board-card="fx-standard"][data-lane="running"]').waitFor()
    assert.equal(await rowSection(page, 'fx-standard'), 'running')
    await page.waitForFunction(() => {
      const active = document.activeElement
      return (
        active?.hasAttribute('data-board-card') && active.getAttribute('data-lane') === 'attention'
      )
    })
    // 收起动画停在透明的行在新分组里原样出现。
    await opacityOf('fx-standard', '1')
    report.checks.push(
      'Standard approval: A on the focused row sends POST /api/sessions/fx-standard/approvals/approval-standard {"approved":true}, the row moves to 运行中 and focus lands on the next needs-you row',
    )

    // 失败行重试：POST /retry（SSE），行进入运行中，侧栏计数跟着变。
    await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname === '/api/sessions/fx-failed/retry',
      ),
      row('fx-failed').getByRole('button', { name: /^重试/ }).click(),
    ])
    await page.locator('[data-board-card="fx-failed"][data-lane="running"]').waitFor()
    assert.equal(fixture.posts.at(-1).path, '/api/sessions/fx-failed/retry')
    await waitForFilterCounts(page, { attention: 1, running: 4 })
    report.checks.push(
      'Failed row 重试 posts /api/sessions/fx-failed/retry, moves to running and the sidebar filter counts follow',
    )

    // 这一页没有任务输入框：⌘N 与页头“新任务”都打开派发对话框，Esc 关闭后留在全部代理。
    const dispatch = page.getByRole('dialog', { name: '新任务', exact: true })
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press(`${mod}+KeyN`)
    await dispatch.waitFor()
    await page.keyboard.press('Escape')
    await dispatch.waitFor({ state: 'hidden' })
    await page.locator('[data-board-root] header').getByRole('button', { name: '新任务' }).click()
    await dispatch.waitFor()
    await page.keyboard.press('Escape')
    await dispatch.waitFor({ state: 'hidden' })
    assert.equal(await page.evaluate(() => location.hash), '#/board')
    report.checks.push(
      '⌘N and the header 新任务 button both open the dispatch dialog; Esc closes it and stays on 全部代理',
    )

    // 副标题悬停提示：子代理 x/4 · 今日 tok · 内存（拿不到的项不显示，不造数字）。
    await page.locator('[data-overview-summary]').focus()
    const system = page.locator('[data-overview-system]').first()
    await system.waitFor()
    await page.waitForFunction(
      () => !/正在读取/.test(document.querySelector('[data-overview-system]')?.textContent || ''),
    )
    assert.match(await system.innerText(), /今日 .+ tok|内存 |子代理 |暂时读不到系统信息/)
    await page.keyboard.press('Escape')
    report.checks.push(
      'The summary line is focusable and its tooltip lists sub-agent slots, today tokens and memory when the runtime reports them',
    )

    // G 切到按项目：每个项目一个分区，标题带各状态计数，区内仍按状态；项目 ⋯ 重命名 / 恢复目录名 / 管理项目。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press('g')
    const pisper = page.locator('[data-overview-heading="/home/dev/pisper"]')
    await pisper.waitFor()
    assert.match(await pisper.innerText(), /pisper[\s\S]*2 个运行中 · 1 个今天完成/)
    assert.equal(
      await page.locator('[data-board-root] [role="radio"][aria-checked="true"]').innerText(),
      '按项目',
    )
    assert.deepEqual(
      await page
        .locator('[data-overview-section="/home/dev/pisper"] [data-board-card]')
        .evaluateAll((rows) => rows.map((node) => node.getAttribute('data-lane'))),
      ['running', 'running', 'done'],
    )
    await pisper.getByRole('button', { name: 'pisper 的操作' }).click()
    for (const name of ['在此项目新建任务', '重命名…', '上移', '下移', '管理项目…', '删除项目…'])
      await page.getByRole('menuitem', { name }).waitFor()
    await page.getByRole('menuitem', { name: '重命名…' }).click()
    const renameDialog = page.getByRole('dialog', { name: '重命名项目' })
    await renameDialog.getByRole('textbox').fill('pisper-app')
    await renameDialog.getByRole('button', { name: '保存' }).click()
    await pisper.getByText('pisper-app', { exact: true }).waitFor()
    await pisper.getByRole('button', { name: 'pisper-app 的操作' }).click()
    await page.getByRole('menuitem', { name: '恢复目录名' }).click()
    await pisper.getByText('pisper', { exact: true }).waitFor()
    await pisper.getByRole('button', { name: 'pisper 的操作' }).click()
    await page.getByRole('menuitem', { name: '管理项目…' }).click()
    const manage = page.getByRole('dialog', { name: '管理项目' })
    await manage.waitFor()
    await manage.getByRole('button', { name: '完成' }).click()
    await manage.waitFor({ state: 'hidden' })
    await page.keyboard.press('g')
    await page.locator('[data-overview-section="attention"]').waitFor()
    report.checks.push(
      'G switches to 按项目: one section per project with per-status counts (rows still sorted by status); the project ⋯ offers new task here / rename / move up / move down / manage / delete, rename and reset the display name work, 管理项目… opens the dialog; G switches back',
    )

    // 行 ⋯ 菜单与侧栏行相同。
    await row('fx-running').hover()
    await row('fx-running')
      .getByRole('button', { name: /的更多操作$/ })
      .click()
    for (const name of ['打开会话', '置顶', '重命名', '复制会话链接', '归档'])
      await page.getByRole('menuitem', { name, exact: true }).waitFor()
    await page.keyboard.press('Escape')
    report.checks.push(
      'The row ⋯ menu matches the sidebar row menu (打开会话 · 置顶 · 重命名 · 复制会话链接 · 归档)',
    )

    // 快捷键 v2：⌘2 功能（更多）、⌘1 全部代理（看板）；组件工作室默认未绑定（⌘3 不再消费）。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press(`${mod}+Digit3`)
    await page.waitForTimeout(300)
    assert.equal(await page.evaluate(() => location.hash), '#/board', '⌘3 is unbound by default')
    for (const [key, hash, entry] of [
      ['Digit2', '#/more', 'features'],
      ['Digit1', '#/board', 'board'],
    ]) {
      await page.keyboard.press(`${mod}+${key}`)
      await page.waitForFunction((expected) => location.hash === expected, hash)
      await page
        .locator(`[data-shell-sidebar] [data-sidebar-entry="${entry}"][aria-current="page"]`)
        .waitFor()
    }
    await page.locator('[data-board-root]').waitFor()
    assert.equal(await page.title(), '全部代理 · Pisper')
    report.checks.push(
      '⌘2 / ⌘1 switch to 功能 and back to 全部代理 (document title 全部代理); ⌘3 is unbound by default',
    )
    await resetBoardView()
    await context.close()
  }

  // 3. 没有需要你：需要你分组留标题与一句安静的空状态；今天完成可折叠再展开（存 pisper-board-view）。
  {
    const { context } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      {
        bridge: desktopBridgeScript,
        sessions: boardFixture().sessions.filter((session) => !session.needsAttention),
      },
    )
    const page = await openBoard(context, 'no attention')
    const attention = page.locator('[data-overview-section="attention"]')
    await attention.getByText('没有等你处理的事', { exact: true }).waitFor()
    assert.equal(await attention.locator('[data-overview-count]').innerText(), '0')
    // 侧栏的需要你筛选计数为 0：照常列出（变灰、不染琥珀）。
    await waitForFilterCounts(page, { attention: 0, running: 2 })
    const doneToggle = page
      .locator('[data-overview-heading="done"]')
      .getByRole('button', { name: /今天完成/ })
    await doneToggle.click()
    assert.equal(await doneToggle.getAttribute('aria-expanded'), 'false')
    assert.equal(await page.locator('[data-board-card="fx-done-unread"]').count(), 0)
    await page.waitForFunction(() =>
      JSON.parse(localStorage.getItem('pisper-board-view') || '{}').state?.collapsedLanes?.includes(
        'done',
      ),
    )
    await geometry(page, 'done collapsed')
    await page.screenshot({ path: join(output, 'board-done-collapsed.png') })
    await doneToggle.click()
    await page.locator('[data-board-card="fx-done-unread"]').waitFor()
    report.checks.push(
      'With nothing needing you the 需要你 heading reads 0 with a quiet empty line (the sidebar filter reads 0 too); 今天完成 collapses and expands again, stored in pisper-board-view',
    )
    // ⌘B 收起为 60px 窄轨：四个入口图标 + 两个计数（需要你 0 变灰不消失、运行中 2）+ 设置；
    // 点运行中计数打开全部代理并筛到运行中（pisper-board-view），侧栏收起时页上写明筛选并可清除；再按 ⌘B 展开。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press(`${mod}+KeyB`)
    const rail = page.locator('[data-shell-sidebar="rail"]')
    await rail.waitFor()
    await page.waitForFunction(
      () => document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect().width === 60,
    )
    const attentionCount = rail.locator('[data-rail-count="attention"]')
    const runningCount = rail.locator('[data-rail-count="running"]')
    await attentionCount.waitFor()
    assert.equal(await attentionCount.getAttribute('aria-label'), '0 个代理需要你，打开全部代理')
    assert.equal(await runningCount.getAttribute('aria-label'), '2 个代理运行中，打开全部代理')
    await geometry(page, 'collapsed rail')
    await page.screenshot({ path: join(output, 'board-rail.png') })
    await runningCount.click()
    await page.waitForFunction(() =>
      JSON.parse(localStorage.getItem('pisper-board-view') || '{}').state?.lanes?.includes(
        'running',
      ),
    )
    assert.equal(await page.locator('[data-board-card="fx-done-unread"]').count(), 0)
    await page.locator('[data-board-card="fx-running"]').waitFor()
    const filterLine = page.locator('[data-overview-filter]')
    assert.match(await filterLine.innerText(), /状态：运行中/)
    await filterLine.getByRole('button', { name: '清除筛选' }).click()
    await page.locator('[data-board-card="fx-done-unread"]').waitFor()
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="expanded"]').waitFor()
    await page.waitForFunction(() =>
      localStorage.getItem('pisper-sidebar')?.includes('"collapsed":false'),
    )
    // 筛选偏好经 Runtime 镜像同步：清掉，免得影响之后的上下文。
    await resetBoardView()
    report.checks.push(
      '⌘B collapses the sidebar to the 60px rail with a greyed 0 needs-you count and 2 running; the running count opens all agents filtered to running with a 状态：运行中 line whose 清除筛选 brings everything back; ⌘B expands it again',
    )
    await context.close()
  }

  // 3b. 今天没有活动（只有 2 天前完成的会话）：一句“还没有代理在运行”+“新任务”与历史入口，
  // 不放“试试”建议，也不退回旧首页式的居中输入框。
  {
    const old = new Date(Date.now() - 2 * 86_400_000).toISOString()
    const sessions = boardFixture()
      .sessions.filter((session) => session.id.startsWith('fx-done'))
      .map((session) => ({
        ...session,
        pinned: false,
        unread: false,
        created: old,
        modified: old,
        lastCompletedAt: old,
      }))
    const { context } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript, sessions },
    )
    const page = await openBoard(context, 'quiet board')
    const empty = page.locator('[data-overview-empty]')
    await empty.getByText('还没有代理在运行', { exact: true }).waitFor()
    assert.equal(await page.locator('[data-board-card], [data-launch-console]').count(), 0)
    assert.equal(await page.getByRole('heading', { name: '还没有任务', exact: true }).count(), 0)
    await empty.getByRole('button', { name: /全部历史会话/ }).waitFor()
    await geometry(page, 'quiet board')
    await page.screenshot({ path: join(output, 'board-quiet.png') })
    await empty.getByRole('button', { name: '新任务' }).click()
    await page.getByRole('dialog', { name: '新任务', exact: true }).waitFor()
    await page.keyboard.press('Escape')
    report.checks.push(
      'No activity today (only sessions finished 2 days ago): one line “还没有代理在运行” with 新任务 (opens the dispatch dialog) and 全部历史会话, no suggestions, no rows and no launch console',
    )
    await context.close()
  }

  // 3c. 超过 30 行改用虚拟列表（规格 3.3）：只渲染可视附近的行；J 走到列表末尾时仍能聚焦到当时不在 DOM 里的行。
  // ⌥A 在列表滚到底时也能把焦点交给顶上（没挂载的）需要你那一行，且不动筛选；当前行被滚出可视窗口时仍挂着，焦点不丢。
  {
    const failed = boardFixture().sessions.find((session) => session.id === 'fx-failed')
    const base = boardFixture().sessions.find((session) => session.id === 'fx-done-unread')
    const many = Array.from({ length: 40 }, (_, index) => ({
      ...base,
      id: `fx-many-${String(index).padStart(2, '0')}`,
      name: `批量任务 ${index + 1}`,
      firstMessage: `批量任务 ${index + 1}`,
      pinned: false,
      unread: false,
    }))
    const { context } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript, sessions: [failed, ...many], protocol: 'legacy' },
    )
    const page = await openBoard(context, 'virtual list')
    await page.locator('[data-overview-list][data-virtual]').waitFor()
    const rendered = await page.locator('[data-board-card]').count()
    assert.ok(rendered > 0 && rendered < 40, `virtual list renders a window of rows: ${rendered}`)
    await page.locator('[data-board-card]').first().focus()
    for (let step = 0; step < 40; step += 1) await page.keyboard.press('j')
    await page
      .waitForFunction(
        () => document.activeElement?.getAttribute('data-board-card') === 'fx-many-39',
        null,
        { timeout: 10_000 },
      )
      .catch(async () => {
        const active = await page.evaluate(() =>
          document.activeElement?.getAttribute('data-board-card'),
        )
        throw new Error(`J did not reach the last row in the virtual list (active: ${active})`)
      })
    // 列表在底部，需要你那一行（fx-failed，在最上面）不在 DOM 里：⌥A 仍把焦点交给它，不清筛选。
    assert.equal(await page.locator('[data-board-card="fx-failed"]').count(), 0)
    const viewBefore = await page.evaluate(() => localStorage.getItem('pisper-board-view'))
    await page.keyboard.press('Alt+KeyA')
    await page
      .waitForFunction(
        () => document.activeElement?.getAttribute('data-board-card') === 'fx-failed',
        null,
        { timeout: 10_000 },
      )
      .catch(async () => {
        const active = await page.evaluate(() =>
          document.activeElement?.getAttribute('data-board-card'),
        )
        throw new Error(
          `⌥A did not reach the needs-you row in the virtual list (active: ${active})`,
        )
      })
    assert.equal(await page.evaluate(() => localStorage.getItem('pisper-board-view')), viewBefore)
    // 滚轮把当前行滚出可视窗口：它仍挂着（唯一 tabIndex=0 的行），焦点不掉到 body 上。
    await page.locator('[data-overview-scroller]').evaluate((node) => {
      node.scrollTop = node.scrollHeight
    })
    await page.waitForTimeout(300)
    assert.equal(
      await page.evaluate(() => document.activeElement?.getAttribute('data-board-card')),
      'fx-failed',
    )
    assert.equal(await page.locator('[data-board-card][tabindex="0"]').count(), 1)
    report.checks.push(
      'More than 30 rows switch to a virtual list that renders a window of rows; J still walks to the last row; with the list at the bottom ⌥A focuses the unmounted needs-you row at the top without touching filters; scrolling the active row out of view keeps it mounted, focused and tabbable',
    )
    await context.close()
  }

  // 4. 手机 App（390×844，规格 v2 第 7.1、7.3 节）：顶栏 ☰ 旁琥珀需要你计数（离开全部代理也在），没有标签栏与桌面标题栏；
  // 需要你是卡片、其余是行。
  for (const colorScheme of ['light', 'dark']) {
    const { context } = await newContext(
      {
        viewport: { width: 390, height: 844 },
        colorScheme,
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 2,
      },
      { mobileApp: true },
    )
    const page = await openBoard(context, `phone ${colorScheme}`)
    const topBar = page.locator('[data-phone-topbar]')
    await topBar.waitFor()
    assert.equal(
      await page.evaluate(() => document.documentElement.hasAttribute('data-phone-shell')),
      true,
    )
    assert.equal(
      await page
        .locator('[data-app-titlebar], [data-testid="mobile-tab-bar"], [data-shell-tab]')
        .count(),
      0,
    )
    const menu = topBar.locator('[data-phone-menu]')
    await page.waitForFunction(
      () =>
        document
          .querySelector('[data-phone-topbar] [data-phone-menu]')
          ?.getAttribute('aria-label') === '打开导航，3 个需要你',
    )
    assert.equal(await menu.locator('[data-phone-attention]').innerText(), '3')
    for (const box of await topBar
      .locator('button')
      .evaluateAll((buttons) => buttons.map((button) => button.getBoundingClientRect().toJSON())))
      assert.ok(box.height >= 44 && box.width >= 44, `top bar touch target ${JSON.stringify(box)}`)
    await page.locator('[data-overview-card]').first().waitFor()
    assert.equal(
      await page
        .locator('[data-overview-card]')
        .evaluateAll((cards) => cards.every((card) => card.dataset.lane === 'attention')),
      true,
    )
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      true,
    )
    await page.screenshot({ path: join(output, `phone-${colorScheme}.png`) })
    // 离开全部代理后，☰ 旁仍带计数。
    await page.goto(page.url().replace(/#.*$/, '#/more'))
    await page.locator('[data-phone-topbar] [data-phone-menu]').waitFor()
    await page.waitForFunction(
      () =>
        document.querySelector('[data-phone-menu]')?.getAttribute('aria-label') ===
        '打开导航，3 个需要你',
    )
    await context.close()
  }
  report.checks.push(
    'Phone app 390x844 (light/dark): 56px top bar with ☰ + amber needs-you count that stays on other pages, 44px touch targets, 需要你 rendered as cards, no tab bar, no desktop title bar, no horizontal scroll',
  )

  // 4b. 代理的提问（ask_user，规格 v2 第 8.4、3.3 节，能力 askUser）：侧栏行写“等你回答”、只能“打开”；
  // 全部代理行写问题正文，一题单选、选项 ≤4 时选项是胶囊，点一下只发一次回答、行离开“需要你”；手机卡片同样点选（44px）。
  for (const phone of [false, true]) {
    const { context, fixture } = await newContext(
      phone
        ? {
            viewport: { width: 390, height: 844 },
            isMobile: true,
            hasTouch: true,
            colorScheme: 'light',
          }
        : { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { mobileApp: phone, sessions: [...boardFixture().sessions, questionFixtureSession()] },
    )
    const page = await openBoard(context, phone ? 'question phone' : 'question desktop')
    const row = page.locator(`[data-board-card="${QUESTION_SESSION_ID}"]`)
    await row.waitFor({ timeout: 30000 })
    assert.equal(await row.getAttribute('data-status'), 'question')
    assert.match(await row.locator('[data-row-question]').innerText(), /会话存储用哪种？/)
    const pills = row.locator('[data-question-pill]')
    assert.deepEqual(
      await pills.evaluateAll((nodes) => nodes.map((node) => node.dataset.questionPill)),
      ['SQLite', 'Postgres'],
    )
    if (phone) {
      assert.equal(await row.getAttribute('data-overview-card'), 'true')
      for (const box of await pills.evaluateAll((nodes) =>
        nodes.map((node) => node.getBoundingClientRect().height),
      ))
        assert.ok(box >= 44, `phone question pill ${box}`)
    } else {
      // 侧栏（全部代理页的侧栏是筛选，换到功能页看会话分组）：等你回答，快捷处理只有“打开”。
      await page.goto(base + '/#/more')
      const sidebarRow = page.locator(`[data-sidebar-row="${QUESTION_SESSION_ID}"]`)
      await sidebarRow.waitFor()
      assert.match(await sidebarRow.getAttribute('aria-label'), /等你回答/)
      assert.match(
        await page
          .locator(`[data-sidebar-item="${QUESTION_SESSION_ID}"] [data-sidebar-quick] button`)
          .first()
          .evaluate((node) => node.textContent),
        /^打开/,
      )
      await page.goto(base + '/#/board')
      await pills.first().waitFor()
    }
    await pills.first().click()
    for (let i = 0; i < 30 && fixture.posts.length < 1; i += 1) await page.waitForTimeout(100)
    await page.waitForTimeout(500)
    assert.deepEqual(
      fixture.posts.map((post) => [post.path, post.body]),
      [
        [
          `/api/sessions/${QUESTION_SESSION_ID}/questions/${QUESTION_ID}`,
          { answers: [{ selected: ['SQLite'] }] },
        ],
      ],
    )
    await page.waitForFunction(
      (id) => document.querySelector(`[data-board-card="${id}"]`)?.dataset.lane !== 'attention',
      QUESTION_SESSION_ID,
      { timeout: 10000 },
    )
    await page.screenshot({ path: join(output, `question-${phone ? 'phone' : 'desktop'}.png`) })
    await context.close()
  }
  report.checks.push(
    'Agent questions (askUser): the sidebar row reads 等你回答 and only offers 打开; the 全部代理 row shows the question and, for one single-select question, option pills (44px on the phone card) where one click sends exactly one answer {selected:[label]} and the row leaves 需要你',
  )

  // 5. 窄化的桌面窗口绝不进入手机外壳：1000 宽，以及低于手机断点的 600 宽；同宽的浏览器作对照。
  for (const [width, bridge] of [
    [1000, plainDesktopBridgeScript],
    [600, plainDesktopBridgeScript],
    [600, desktopBridgeScript],
    [600, null],
  ]) {
    const label = `desktop gate ${width} ${bridge ? 'desktop' : 'browser'}`
    const { context } = await newContext(
      { viewport: { width, height: 760 }, colorScheme: 'light' },
      { bridge },
    )
    const page = await openBoard(context, label)
    await settle(page)
    const phoneShell = await page.evaluate(() =>
      document.documentElement.hasAttribute('data-phone-shell'),
    )
    if (bridge) {
      assert.equal(phoneShell, false, `${label}: desktop window must not enter the phone shell`)
      assert.equal(await page.locator('[data-phone-topbar], [data-phone-drawer]').count(), 0)
      assert.equal(await page.locator('[data-shell-sidebar]').isVisible(), true)
    } else {
      // 对照组：普通浏览器低于手机断点时进入手机外壳，证明上面的门控确实生效。
      assert.equal(phoneShell, true, `${label}: a phone-width browser uses the phone shell`)
      await page.locator('[data-phone-topbar]').waitFor()
    }
    await context.close()
  }
  report.checks.push(
    'Desktop windows narrowed to 1000px and 600px (with and without the custom title bar) keep the sidebar shell and never mount the phone shell; a 600px browser does',
  )

  // 6. 第 2 期数据源：Runtime 声明 board.view / board.events 时全部代理走事件流，行显示 card 摘要
  // （运行中的当前活动、失败摘要、今天完成的结果摘要与改动统计）；旧桌面 Runtime（能力里没有 board.*）
  // 退回第 1 期路径且没有错误。
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'board feed')
    await page.locator('[data-board-card="fx-running"] [data-card-activity]').waitFor()
    assert.ok(fixture.eventRequests.length >= 1, 'the board opens the event stream')
    assert.equal(fixture.eventRequests[0], null, 'the first connection asks for a snapshot')
    assert.match(
      await page.locator('[data-board-card="fx-running"] [data-card-activity]').innerText(),
      /bash · npm test -- --runInBand/,
    )
    assert.match(
      await page.locator('[data-board-card="fx-failed"] [data-card-error]').innerText(),
      /3 failed, 41 passed/,
    )
    assert.match(
      await page.locator('[data-board-card="fx-done-unread"] [data-card-preview]').innerText(),
      /LCP/,
    )
    assert.match(
      await page.locator('[data-board-card="fx-done-unread"]').innerText(),
      /6 个文件\s*\+214\s*−97/,
    )
    // 有 card 时不再为审批行单独取 /live，也不再轮询完整目录之外的 /change-summary。
    const lateRequests = []
    page.on('request', (request) => lateRequests.push(new URL(request.url()).pathname))
    await page.waitForTimeout(2_500)
    assert.equal(
      lateRequests.filter((path) => /\/live$|\/change-summary$/.test(path)).length,
      0,
      `no /live or /change-summary while cards carry the details: ${lateRequests.join(', ')}`,
    )
    // 带 after 重连（固定数据每次连接发完就结束，前端按 1 秒间隔重连）。
    assert.ok(
      fixture.eventRequests.some((after) => after !== null),
      `reconnects carry after: ${JSON.stringify(fixture.eventRequests)}`,
    )
    await geometry(page, 'board feed')
    await context.close()
  }
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript, protocol: 'legacy' },
    )
    const requests = []
    context.on('request', (request) => requests.push(new URL(request.url())))
    const page = await openBoard(context, 'old runtime')
    await waitForApprovals(page)
    await page.locator('[data-board-card="fx-running"][data-lane="running"]').waitFor()
    assert.equal(fixture.eventRequests.length, 0)
    assert.equal(
      requests.filter((url) => url.pathname === '/api/board/events').length,
      0,
      'an old runtime never gets a board event stream request',
    )
    assert.equal(
      requests.filter((url) => url.searchParams.get('view') === 'board').length,
      0,
      'an old runtime never gets view=board',
    )
    assert.ok(
      requests.some((url) => /\/live$/.test(url.pathname)),
      'approval rows use /live',
    )
    // 没有 card：运行中不造活动行，今天完成的改动统计只为可视区内的行懒读 /change-summary。
    assert.equal(
      await page.locator('[data-board-card="fx-running"] [data-card-activity]').count(),
      0,
    )
    await page
      .locator('[data-board-card="fx-done-unread"]')
      .getByText('6 个文件')
      .waitFor({ timeout: 15_000 })
    await context.close()
  }
  report.checks.push(
    'Board feed: with board.view/board.events the page opens the event stream (snapshot first, then reconnects with after), shows the running activity, failure summary, done preview with change stats, and takes approvals from cards without /live or /change-summary; an old runtime without board.* falls back to /api/sessions + /live with no event-stream or view=board requests, no invented activity line, and lazily reads change stats',
  )

  // 6b. 真实 Runtime 的看板事件流（不经固定数据）：每个标签页只有 1 条长连接，先收快照、随后收增量；
  // 去会话页再回来仍是同一条；页面隐藏时断开，回到可见后带 after 重连。页面可见性用改写
  // visibilityState 模拟（浏览器模拟，不等于 WebView 或真机的前后台切换）。
  {
    const hub = app.runtime?.boardEvents
    assert.ok(hub, 'the runtime exposes its board event hub')
    const context = await browser.newContext({
      locale: 'zh-CN',
      viewport: { width: 1440, height: 900 },
      colorScheme: 'light',
    })
    await context.addInitScript(() => {
      try {
        localStorage.setItem('pisper-language', 'zh-CN')
        localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
      } catch {}
      let hidden = false
      Object.defineProperty(Document.prototype, 'visibilityState', {
        configurable: true,
        get: () => (hidden ? 'hidden' : 'visible'),
      })
      Object.defineProperty(Document.prototype, 'hidden', { configurable: true, get: () => hidden })
      window.__setHidden = (value) => {
        hidden = value
        document.dispatchEvent(new Event('visibilitychange'))
      }
    })
    await context.route('**/api/app-update*', (route) =>
      route.fulfill({ json: { state: 'current', checkedAt: Date.now() } }),
    )
    const streams = []
    context.on('request', (request) => {
      const url = new URL(request.url())
      if (url.pathname === '/api/board/events') streams.push(url.searchParams.get('after'))
    })
    const connections = () => hub.stats().connections
    const until = async (predicate, label, timeout = 15_000) => {
      const deadline = Date.now() + timeout
      while (!predicate()) {
        if (Date.now() > deadline) throw new Error(`timed out: ${label}`)
        await new Promise((resolve) => setTimeout(resolve, 100))
      }
    }
    const page = await openBoard(context, 'live board events')
    const card = page.locator(`[data-board-card="${BOARD_SESSION_ID}"]`)
    await card.waitFor({ timeout: 20_000 })
    await until(() => connections() === 1, 'one live board stream')
    await page.waitForTimeout(2_500)
    assert.equal(connections(), 1, 'one tab keeps exactly one board event stream')
    assert.equal(streams.length, 1, `no reconnect while the stream is healthy: ${streams}`)
    assert.equal(streams[0], null, 'the first connection asks for a snapshot')
    // 增量：置顶经真实接口写入，事件流推送 session_card，卡片出现置顶标记（不靠目录轮询）。
    const before = streams.length
    const pin = await fetch(`${base}/api/sessions/${BOARD_SESSION_ID}/organization`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pinned: true }),
    })
    assert.equal(pin.ok, true)
    await card.getByLabel('已置顶', { exact: true }).waitFor({ timeout: 10_000 })
    assert.equal(streams.length, before, 'the increment arrives on the same connection')
    // 会话页与返回：看板 store 在路由间共享，不新开第二条连接。
    await card.click({ position: { x: 60, y: 14 } })
    await page.locator('[data-session-header]').first().waitFor({ timeout: 30_000 })
    await page.waitForTimeout(1_000)
    assert.equal(connections(), 1, 'the session page reuses the board stream')
    await page.keyboard.press(`${mod}+Digit1`)
    await page.locator('[data-board-root]').waitFor()
    await page.waitForTimeout(1_000)
    assert.equal(connections(), 1, 'returning to the board keeps one stream')
    // 隐藏断开、可见后带 after 重连。
    await page.evaluate(() => window.__setHidden(true))
    await until(() => connections() === 0, 'hidden page closes the stream')
    const hiddenAt = streams.length
    await page.waitForTimeout(1_500)
    assert.equal(streams.length, hiddenAt, 'a hidden page does not reconnect')
    await page.evaluate(() => window.__setHidden(false))
    await until(() => connections() === 1, 'visible page reconnects')
    const resumed = streams.at(-1)
    assert.ok(resumed !== null && /^\d+$/.test(resumed), `reconnects with after: ${resumed}`)
    const unpin = await fetch(`${base}/api/sessions/${BOARD_SESSION_ID}/organization`, {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ pinned: false }),
    })
    assert.equal(unpin.ok, true)
    await card.getByLabel('已置顶', { exact: true }).waitFor({ state: 'detached', timeout: 10_000 })
    await context.close()
    await until(() => connections() === 0, 'closing the tab releases the stream')
  }
  report.checks.push(
    'Live runtime board events: one tab holds exactly one /api/board/events connection (snapshot first, a pin increment arrives on the same connection, the session page and the way back reuse it); hiding the page closes it and becoming visible reconnects with after',
  )

  // 7. 第 3 期会话页：真实会话（隔离 Runtime 里的 JSONL）放进固定数据的“运行中”泳道。
  // /live 取真实 Runtime 的结果，只把运行状态改成运行中；停止请求在浏览器边界记录，不发给 Runtime。
  const realList = await fetch(`${base}/api/sessions`).then((response) => response.json())
  const realItem = realList.sessions.find((session) => session.id === BOARD_SESSION_ID)
  assert.ok(realItem, 'the isolated runtime lists the session-page fixture')
  const sessionBoard = [
    ...boardFixture().sessions,
    { ...realItem, pinned: false, unread: false, needsAttention: false, streaming: true },
  ]
  const startedAt = new Date(Date.now() - 12_000).toISOString()
  async function sessionContext(options, extra = {}) {
    const { context, fixture } = await newContext(options, { ...extra, sessions: sessionBoard })
    const aborts = []
    await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/live`), async (route) => {
      const response = await route.fetch()
      const live = await response.json().catch(() => ({}))
      await route.fulfill({
        response,
        json: { ...live, streaming: true, startedAt, lastActivityAt: new Date().toISOString() },
      })
    })
    await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/abort$`), async (route) => {
      aborts.push(route.request().method())
      await route.fulfill({ json: { ok: true } })
    })
    return { context, fixture, aborts }
  }
  async function openSession(page) {
    await page.goto(`${base}/#/board/${BOARD_SESSION_ID}`)
    await page.locator('.focus-composer textarea').first().waitFor({ timeout: 30_000 })
    await page.getByText('已完成重构，全部测试通过。').first().waitFor({ timeout: 30_000 })
  }
  // 几何：横向滚动、右上 144px、页头的高度，侧栏入口与检查器里的控件没有越出视口、
  // 也没有被别的元素挡住（取控件中心点命中测试）。
  // 覆盖层打开时盖住对话列右侧（含页头右端）是设计本身，那时只核对侧栏与检查器里的控件。
  function sessionGeometry(page, { header = true } = {}) {
    return page.evaluate((includeHeader) => {
      const rect = (selector) => document.querySelector(selector)?.getBoundingClientRect() ?? null
      const header = rect('[data-session-header]')
      const overlay = rect('[data-inspector-overlay]')
      const blocked = [
        ...document.querySelectorAll(
          '[data-session-inspector] button, [data-session-inspector] [role="tab"], [data-shell-sidebar] [data-sidebar-entry]' +
            (includeHeader ? ', [data-session-header] button' : ''),
        ),
      ]
        .filter((element) => {
          const box = element.getBoundingClientRect()
          if (!box.width || !box.height || getComputedStyle(element).visibility === 'hidden')
            return false
          if (box.right > innerWidth + 0.5 || box.bottom > innerHeight + 0.5 || box.left < -0.5)
            return true
          const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2)
          return Boolean(hit && !element.contains(hit) && !hit.contains(element))
        })
        .map((element) => element.getAttribute('aria-label') || element.textContent?.trim())
      const intrusions = window.pisperDesktop?.customTitlebar
        ? [...document.querySelectorAll('button, a[href], input, [role="tab"]')]
            .filter((element) => !element.closest('.desktop-window-controls'))
            .filter((element) => {
              const box = element.getBoundingClientRect()
              return box.width > 0 && box.top < 48 && box.right > innerWidth - 144 + 0.5
            })
            .map((element) => element.getAttribute('aria-label') || element.textContent?.trim())
        : []
      return {
        horizontalScroll:
          document.documentElement.scrollWidth > innerWidth ||
          document.body.scrollWidth > innerWidth,
        headerBottom: header ? Math.round(header.bottom) : null,
        headerHeight: header ? Math.round(header.height) : null,
        sidebarWidth: Math.round(
          document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect().width ?? 0,
        ),
        workspace:
          document
            .querySelector('[data-session-workspace]')
            ?.getAttribute('data-session-workspace') ?? null,
        overlay: overlay
          ? {
              top: Math.round(overlay.top),
              right: Math.round(overlay.right),
              bottom: Math.round(overlay.bottom),
              width: Math.round(overlay.width),
            }
          : null,
        blocked,
        intrusions,
      }
    }, header)
  }

  // 7a. 1440×900：侧栏（当前会话行、快捷处理、⌘J、⌥A）、⌘I、⌥↑/⌥↓、Esc、⌘1（⌘[ 已退役，不再离开会话页）。
  {
    const { context, fixture, aborts } = await sessionContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'session page')
    await waitForApprovals(page)
    const realCard = page.locator(`[data-board-card="${BOARD_SESSION_ID}"][data-lane="running"]`)
    await realCard.waitFor()
    await settle(page)
    // 打开一个本客户端不持有运行流的运行中会话时，转录先显示加载态、立即读 /live，
    // 不能先闪出空会话的问候页（旧工作台的“夜深啦…”）再跳成对话。
    await page.evaluate(() => {
      window.__greetingSeen = false
      const observer = new MutationObserver(() => {
        if (document.querySelector('[data-testid="workbench-greeting"]'))
          window.__greetingSeen = true
      })
      observer.observe(document.body, { childList: true, subtree: true })
      window.__greetingObserver = observer
    })
    // 从卡片打开（点标题区域，避开卡片上的按钮），之后 ⌘1 回到看板要把焦点还给这张卡片。
    const openedAt = Date.now()
    await realCard.click({ position: { x: 60, y: 14 } })
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    const sidebar = page.locator('[data-shell-sidebar]')
    const sidebarRow = (id) => sidebar.locator(`[data-sidebar-row="${id}"]`)
    const sidebarItem = (id) => sidebar.locator(`[data-sidebar-item="${id}"]`)
    await page.locator('[data-run-bar="running"]').waitFor({ timeout: 30_000 })
    // 会话页没有泳道条；当前会话在侧栏里是带 aria-current 的行（运行中分组，带实时计时）。
    await sidebar
      .locator(`[data-sidebar-row="${BOARD_SESSION_ID}"][aria-current="page"]`)
      .waitFor({ timeout: 30_000 })
    assert.equal(await page.locator('[data-lane-strip], [data-agent-panel]').count(), 0)
    // 转录来自真实 Runtime：从看板卡片打开也要读到这条会话自己的历史。
    await page.getByText('已完成重构，全部测试通过。').first().waitFor({ timeout: 30_000 })
    const transcriptAfter = Date.now() - openedAt
    const greetingSeen = await page.evaluate(() => {
      window.__greetingObserver?.disconnect()
      return window.__greetingSeen
    })
    assert.equal(greetingSeen, false, 'opening a running session never flashes the empty greeting')
    report.checks.push(
      `Opening a running session from its card shows loading, never the empty-session greeting; the conversation appeared after ${transcriptAfter}ms`,
    )
    await settle(page)
    const geo = await sessionGeometry(page)
    assert.equal(geo.horizontalScroll, false, 'session page: no horizontal scroll')
    assert.equal(geo.sidebarWidth, 268, `session page: 268px sidebar ${JSON.stringify(geo)}`)
    assert.equal(
      geo.headerHeight,
      52,
      'session page: 52px session header (aligned with the sidebar brand row)',
    )
    assert.equal(geo.workspace, 'aside', 'wide session page docks the inspector')
    // 规格 v2 第 4.4 节：检查器默认关着、按需滑入（没有固定时）；对话列独占主区。
    assert.equal(
      await page.locator('[data-session-inspector]').count(),
      0,
      'the inspector starts closed',
    )
    assert.deepEqual(geo.intrusions, [], 'session page: top-right 144px stays free')
    assert.equal(await page.locator('[data-session-title]').innerText(), BOARD_SESSION_TITLE)
    await page.screenshot({ path: join(output, 'session-1440.png') })

    // critical 行只有“打开”（进会话按住确认），A 不批准；standard 行悬停后一步允许：一次 POST，
    // 请求体与看板一致，行 160ms 收起后离开需要你分组。
    await sidebarRow('fx-critical').hover()
    await sidebarItem('fx-critical').getByRole('button', { name: /^打开/ }).waitFor()
    assert.equal(await sidebarItem('fx-critical').getByRole('button', { name: /^允许/ }).count(), 0)
    await sidebarRow('fx-critical').focus()
    await page.keyboard.press('a')
    await page.waitForTimeout(400)
    assert.equal(fixture.posts.length, 0, 'A on a critical sidebar row never approves')
    assert.equal(page.url().endsWith(`#/board/${BOARD_SESSION_ID}`), true)
    await sidebarRow('fx-standard').hover()
    const sidebarApprove = sidebarItem('fx-standard').getByRole('button', { name: /^允许/ })
    await sidebarApprove.waitFor()
    const [approveResponse] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname ===
            '/api/sessions/fx-standard/approvals/approval-standard',
      ),
      sidebarApprove.click(),
    ])
    assert.equal(approveResponse.request().postData(), JSON.stringify({ approved: true }))
    assert.deepEqual(fixture.posts, [
      {
        method: 'POST',
        path: '/api/sessions/fx-standard/approvals/approval-standard',
        body: { approved: true },
      },
    ])
    await page.waitForFunction(
      () =>
        !document.querySelector(
          '[data-sidebar-group="attention"] [data-sidebar-row="fx-standard"]',
        ),
    )
    assert.deepEqual(aborts, [], 'sidebar actions never stop the current run')
    assert.equal(page.url().endsWith(`#/board/${BOARD_SESSION_ID}`), true)
    report.checks.push(
      'Session page sidebar: the critical row only offers 打开 and A on it sends nothing; hovering the standard row reveals 允许, one click sends one POST {"approved":true} and the row leaves 需要你 while the session stays open',
    )

    // ⌘J：从输入框把焦点移到侧栏的当前会话行，↓ 在行间移动；⌥A 聚焦等待最久的需要你行（不批准）。
    const prompt = page.locator('.focus-composer textarea')
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForFunction(
      (id) => document.activeElement?.getAttribute('data-sidebar-row') === id,
      BOARD_SESSION_ID,
    )
    const focusedRow = () =>
      page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-row') ?? null)
    const before = await focusedRow()
    await page.keyboard.press('ArrowDown')
    await page.waitForFunction(
      (id) => document.activeElement?.getAttribute('data-sidebar-row') !== id,
      before,
    )
    assert.ok(await focusedRow(), 'ArrowDown moves to the next sidebar row')
    await prompt.click()
    await page.keyboard.press('Alt+KeyA')
    await page.waitForFunction(
      () =>
        document.activeElement?.getAttribute('data-sidebar-group-row') === 'attention' &&
        document.activeElement ===
          document.querySelector('[data-sidebar-group="attention"] [data-sidebar-row]'),
    )
    assert.equal(fixture.posts.length, 1, '⌥A only moves focus')
    assert.equal(page.url().endsWith(`#/board/${BOARD_SESSION_ID}`), true)
    assert.deepEqual(aborts, [], '⌘J / ⌥A never stop the run')
    report.checks.push(
      '⌘J moves focus from the composer to the current session row in the sidebar and ↓ walks the rows; ⌥A focuses the longest-waiting needs-you row without approving or navigating',
    )

    // ⌘I：输入框内也能开合检查器；宽屏是推开对话的停靠面板（456px），焦点留在输入框里。
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-inspector-aside] [data-session-inspector]').waitFor()
    assert.equal(
      await page.evaluate(() => document.activeElement?.matches('.focus-composer textarea')),
      true,
      '⌘I on the wide page keeps focus in the composer',
    )
    assert.equal(
      await page.locator('[data-inspector-toggle]').getAttribute('aria-expanded'),
      'true',
    )
    // 模拟 Windows 自绘标题栏：停靠面板从顶栏下方开始，每个标签与“+”、收起键都完整可见、可点，
    // 不在右上 144px 里（评审返修：标签行曾让出 144px，“预览”和“+”被裁掉且没有滚动提示）。
    await settle(page)
    const inspectorControls = await page.evaluate(() => {
      const aside = document.querySelector('[data-inspector-aside]')
      const list = aside.querySelector('[role="tablist"]').getBoundingClientRect()
      return [...aside.querySelectorAll('header [role="tab"], header button')].map((element) => {
        const box = element.getBoundingClientRect()
        const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2)
        return {
          name: element.getAttribute('aria-label') || element.textContent?.trim(),
          inTablist: Boolean(element.closest('[role="tablist"]')),
          clipped: element.closest('[role="tablist"]')
            ? box.right > list.right + 0.5 || box.left < list.left - 0.5
            : false,
          corner: box.top < 48 && box.right > innerWidth - 144 + 0.5,
          covered: !(hit && (element.contains(hit) || hit.contains(element))),
        }
      })
    })
    assert.ok(inspectorControls.length >= 5, JSON.stringify(inspectorControls))
    for (const control of inspectorControls)
      assert.ok(
        !control.clipped && !control.corner && !control.covered,
        `inspector control reachable under the custom title bar: ${JSON.stringify(control)}`,
      )
    // 未固定的停靠面板是按需滑入的：Esc 先收起它（不停止运行），焦点留在输入框；
    // 点改动按钮打开后 Esc 同样只收起，焦点回到改动按钮。
    await page.keyboard.press('Escape')
    await page.locator('[data-session-inspector]').waitFor({ state: 'detached' })
    assert.equal(
      await page.evaluate(() => document.activeElement?.matches('.focus-composer textarea')),
      true,
      'Esc closing the docked inspector keeps focus in the composer',
    )
    await page.locator('[data-inspector-toggle]').click()
    await page.locator('[data-inspector-aside] [data-session-inspector]').waitFor()
    await page.keyboard.press('Escape')
    await page.locator('[data-session-inspector]').waitFor({ state: 'detached' })
    assert.equal(
      await page.evaluate(() => document.activeElement?.hasAttribute('data-inspector-toggle')),
      true,
      'Esc returns focus to the changes button',
    )
    await page.waitForTimeout(300)
    assert.deepEqual(
      aborts,
      [],
      'Esc on an open docked inspector closes it and never stops the run',
    )
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-inspector-aside] [data-session-inspector]').waitFor()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-session-inspector]').waitFor({ state: 'detached' })
    report.checks.push(
      'The inspector starts closed on the wide session page; ⌘I slides the docked inspector in and out from inside the composer and the changes button reports aria-expanded',
      'With the simulated Windows title bar the docked inspector starts below the 52px header band: every tab, + and hide stay fully visible, uncovered and out of the top-right 144px',
      'Esc on the unpinned docked inspector closes it first (focus stays in the composer, or returns to the changes button) without stopping the run; the next Esc stops it',
    )

    // ⌥↓ / ⌥↑：按侧栏顺序换会话，只换路由（草稿按会话保留），浏览器后退回到看板。
    await prompt.fill('会话页的未发送草稿')
    await page.keyboard.press('Alt+ArrowDown')
    await page.waitForFunction((id) => !location.hash.endsWith(id), BOARD_SESSION_ID)
    const next = await page.evaluate(() => location.hash)
    assert.match(next, /^#\/board\/fx-/, 'Alt+ArrowDown opens the next session in the sidebar')
    await page.locator('[data-session-header]').first().waitFor()
    await page.keyboard.press('Alt+ArrowUp')
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    await prompt.waitFor()
    await page.waitForFunction(
      () => document.querySelector('.focus-composer textarea')?.value === '会话页的未发送草稿',
    )
    await prompt.fill('')
    // 连按：路由在 transition 里提交，⌥↓ 的新会话还没换上来时就按 ⌥↑，要从刚发出的目标算起，
    // 回到原会话（曾偶发落到原会话的上一个代理，或被当成“已在原会话”丢掉）。
    await page.keyboard.press('Alt+ArrowDown')
    await page.keyboard.press('Alt+ArrowUp')
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    await sidebar.locator(`[data-sidebar-row="${BOARD_SESSION_ID}"][aria-current="page"]`).waitFor()
    assert.equal(
      await page.evaluate(() => location.hash),
      `#/board/${BOARD_SESSION_ID}`,
      'back-to-back ⌥↓ ⌥↑ returns to the starting session',
    )
    report.checks.push(
      '⌥↓ / ⌥↑ switch sessions along the sidebar order by route and keep each session draft; back-to-back presses start from the switch that has not committed yet',
    )

    // 运行中按 Esc：只停止当前运行（POST /abort 一次），不离开会话页。
    await prompt.click()
    await page.keyboard.press('Escape')
    const deadline = Date.now() + 5_000
    while (!aborts.length && Date.now() < deadline) await page.waitForTimeout(100)
    assert.deepEqual(aborts, ['POST'], 'Esc while running stops the run once')
    await page.waitForTimeout(300)
    assert.equal(
      await page.evaluate(() => location.hash),
      `#/board/${BOARD_SESSION_ID}`,
      'Esc never leaves the session page',
    )
    report.checks.push('Esc while running sends one POST /abort and stays on the session page')

    // ⌘[ 已退役：不再离开会话页。
    await page.keyboard.press(`${mod}+BracketLeft`)
    await page.waitForTimeout(400)
    assert.equal(
      await page.evaluate(() => location.hash),
      `#/board/${BOARD_SESSION_ID}`,
      '⌘[ no longer leaves the session page',
    )
    // ⌘1：打开看板（全部代理），焦点落在打开它的卡片上（⇧⌘B 在 v2 归会话树）。
    await page.keyboard.press(`${mod}+Digit1`)
    await page.locator('[data-board-root]').waitFor()
    await page.waitForFunction(
      (id) =>
        document.activeElement?.closest('[data-board-card]')?.getAttribute('data-board-card') ===
        id,
      BOARD_SESSION_ID,
      { timeout: 10_000 },
    )
    report.checks.push(
      '⌘[ no longer navigates; ⌘1 opens the board and focuses the card the session was opened from',
    )
    await context.close()
  }

  // 7b. 1080×680 + 模拟 Windows 自绘标题栏：侧栏照常展开（268 + 812 主区），检查器是覆盖层（默认关闭），
  // 打开后在视口内、控件无裁切无遮挡、无横向滚动；Esc 先关覆盖层，不停止运行也不离开。
  {
    const { context, aborts } = await sessionContext(
      { viewport: { width: 1080, height: 680 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'session 1080')
    await openSession(page)
    await page.locator('[data-run-bar="running"]').waitFor({ timeout: 30_000 })
    await settle(page)
    let geo = await sessionGeometry(page)
    assert.equal(geo.sidebarWidth, 268, `1080: the sidebar stays expanded ${JSON.stringify(geo)}`)
    assert.ok(geo.headerBottom !== null && geo.headerBottom <= 128, `1080: chrome ≤ 128px`)
    assert.equal(geo.workspace, 'overlay')
    assert.equal(await page.locator('[data-session-inspector]').count(), 0, 'overlay starts closed')
    assert.equal(geo.horizontalScroll, false)
    assert.deepEqual(geo.intrusions, [], '1080: top-right 144px stays free')
    assert.deepEqual(geo.blocked, [], '1080: no clipped or covered controls')
    // 规格 4.4 / 9.2：覆盖层无遮罩，用户打开（⌘I 或改动按钮）时焦点进面板的当前标签，Tab / ⇧Tab 在面板里循环，
    // Esc 先关覆盖层（不停止运行），焦点回到顶栏的改动按钮。
    const composerInput = page.locator('.focus-composer textarea').first()
    const focusInOverlay = () =>
      page.evaluate(() => Boolean(document.activeElement?.closest('[data-inspector-overlay]')))
    await composerInput.click()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-inspector-overlay]').waitFor()
    await page.waitForFunction(() =>
      document.activeElement?.matches(
        '[data-inspector-overlay] [role="tab"][aria-selected="true"]',
      ),
    )
    for (let step = 0; step < 24; step += 1) {
      await page.keyboard.press('Tab')
      assert.equal(await focusInOverlay(), true, `Tab ${step + 1} stays inside the overlay`)
    }
    for (let step = 0; step < 24; step += 1) {
      await page.keyboard.press('Shift+Tab')
      assert.equal(await focusInOverlay(), true, `Shift+Tab ${step + 1} stays inside the overlay`)
    }
    await page.keyboard.press('Escape')
    await page.locator('[data-inspector-overlay]').waitFor({ state: 'detached' })
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-inspector-toggle'))
    assert.deepEqual(aborts, [], 'Esc closes the overlay and does not stop the run')
    report.checks.push(
      '1080x680 overlay focus: ⌘I from the composer moves focus to the selected inspector tab, 24 × Tab and 24 × Shift+Tab stay inside the overlay, Esc closes it without stopping the run and returns focus to the changes button',
    )
    await page.locator('[data-inspector-toggle]').click()
    await page.locator('[data-inspector-overlay]').waitFor()
    await page.locator('[data-session-inspector]').waitFor()
    await settle(page)
    // 覆盖层从顶栏下方开始，顶栏按钮都能点到。侧栏常驻后 1080 宽窗口的主区只有 812px，
    // 让出 420px 后对话列不足 480，按规格 4.4 是纯覆盖层（不推开对话）：这时停止键与发送键在覆盖层下面，
    // Esc 先关覆盖层（见上方）。
    const reachable = await page.evaluate(() =>
      ['[data-inspector-toggle]', '[data-session-header] [id$="-more"]'].map((selector) => {
        const element = document.querySelector(selector)
        const box = element?.getBoundingClientRect()
        if (!element || !box?.width) return [selector, 'missing']
        const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2)
        return [selector, Boolean(hit && (element === hit || element.contains(hit)))]
      }),
    )
    assert.deepEqual(
      reachable.filter(([, ok]) => ok !== true),
      [],
      `1080 overlay leaves the header controls reachable ${JSON.stringify(reachable)}`,
    )
    assert.equal(
      await page.locator('[data-overlay-inset]').count(),
      0,
      '812px main area: the overlay does not squeeze the conversation below 480px',
    )
    geo = await sessionGeometry(page)
    assert.ok(
      geo.overlay &&
        geo.overlay.width === 420 &&
        geo.overlay.right <= 1080 &&
        geo.overlay.bottom <= 680 &&
        geo.overlay.top >= 48,
      `1080: overlay inside the viewport below the title bar ${JSON.stringify(geo.overlay)}`,
    )
    assert.equal(
      await page.locator('[data-slot="sheet-overlay"], [data-slot="dialog-overlay"]').count(),
      0,
      'the inspector overlay has no scrim',
    )
    assert.equal(geo.horizontalScroll, false, '1080: no horizontal scroll with the overlay open')
    assert.deepEqual(geo.intrusions, [], '1080 overlay: top-right 144px stays free')
    assert.deepEqual(geo.blocked, [], '1080 overlay: no clipped or covered controls')
    await page.screenshot({ path: join(output, 'session-1080-overlay.png') })
    // 焦点无主（例如点了不可聚焦的转录文字）：Esc 关闭时同样把焦点交回改动按钮。
    await page.evaluate(() => document.activeElement?.blur())
    assert.equal(await page.evaluate(() => document.activeElement === document.body), true)
    await page.keyboard.press('Escape')
    await page.locator('[data-inspector-overlay]').waitFor({ state: 'detached' })
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-inspector-toggle'))
    await page.waitForTimeout(300)
    assert.deepEqual(aborts, [], 'Esc closes the overlay first and does not stop the run')
    assert.equal(await page.evaluate(() => location.hash), `#/board/${BOARD_SESSION_ID}`)
    await context.close()
  }
  report.checks.push(
    '1080x680 with the simulated custom title bar: the 268px sidebar stays expanded, chrome ≤ 128px, the inspector is a 420px overlay without a scrim below the session header (with the 812px main area it stays a pure overlay instead of squeezing the conversation below 480px) and header controls stay reachable, nothing clipped, covered or in the top-right 144px, no horizontal scroll; Esc closes the overlay without stopping the run and focus returns to the changes button',
  )

  // 7c. 手机 App 390×844：第 4 期手机会话页（顶栏左侧“看板 ▲N”入口、无泳道条、隐藏底部标签栏），
  // 输入框可用；完整的代理条 / 底部表单 / 返回键覆盖在 smoke-mobile-shell。
  {
    const { context } = await sessionContext(
      {
        viewport: { width: 390, height: 844 },
        colorScheme: 'light',
        isMobile: true,
        hasTouch: true,
        deviceScaleFactor: 2,
      },
      { mobileApp: true },
    )
    const page = await openBoard(context, 'session phone')
    await openSession(page)
    const phone = await page.evaluate(() => ({
      strip: document.querySelectorAll('[data-lane-strip]').length,
      shell: document.querySelector('[data-session-shell]')?.getAttribute('data-session-shell'),
      tabBar: Boolean(document.querySelector('[data-testid="mobile-tab-bar"]')),
      horizontalScroll: document.documentElement.scrollWidth > innerWidth,
    }))
    assert.deepEqual(phone, { strip: 0, shell: 'phone', tabBar: false, horizontalScroll: false })
    const prompt = page.locator('.focus-composer textarea')
    assert.equal(await prompt.isVisible(), true)
    await prompt.fill('手机会话页的草稿')
    assert.equal(await prompt.inputValue(), '手机会话页的草稿')
    // 回全部代理：☰ 打开抽屉（规格 7.1，取代第 4 期的“看板 N”入口），入口 44px。
    const menu = page.locator('[data-session-header="phone"] [data-phone-menu]')
    const menuBox = await menu.boundingBox()
    assert.ok(menuBox && menuBox.height >= 44, `phone menu target ${JSON.stringify(menuBox)}`)
    await page.screenshot({ path: join(output, 'session-phone.png') })
    await menu.click()
    await page.locator('[data-phone-drawer] [data-drawer-entry="board"]').click()
    await page.locator('[data-board-root]').waitFor()
    await context.close()
  }
  report.checks.push(
    'Phone app 390x844: the phone session page (no lane strip, no bottom tab bar, composer usable, 44px ☰ that opens the drawer back to 全部代理, no horizontal scroll)',
  )

  // 7d. 会话页头“⋯ › 重命名会话”（旧侧栏的右键重命名随外壳退役）：空白名不能保存、取消不发请求；
  // 保存经真实 Runtime 的 PATCH，页头标题与 Runtime 同步。这一段会话不在运行（运行中不允许改名）。
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      {
        bridge: desktopBridgeScript,
        sessions: [
          ...boardFixture().sessions,
          { ...realItem, pinned: false, unread: false, needsAttention: false, streaming: false },
        ],
      },
    )
    // /live 直接交给真实 Runtime（空闲）；改名请求转发给 Runtime 后同步写进看板目录的固定数据，
    // 这样之后的目录刷新不会把标题改回去。
    await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/live`), (route) =>
      route.continue(),
    )
    const renames = []
    await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}$`), async (route) => {
      if (route.request().method() !== 'PATCH') return route.fallback()
      const response = await route.fetch()
      const body = await response.json().catch(() => ({}))
      renames.push({ status: response.status(), name: body.name })
      if (response.ok()) fixture.update(BOARD_SESSION_ID, { name: body.name })
      await route.fulfill({ response, json: body })
    })
    const page = await openBoard(context, 'session rename')
    await page.goto(`${base}/#/board/${BOARD_SESSION_ID}`)
    const title = page.locator('[data-session-header] [data-session-title]')
    await title.filter({ hasText: BOARD_SESSION_TITLE }).waitFor({ timeout: 30_000 })
    const openRename = async () => {
      await page.locator('[data-session-header] [data-session-menu]').click()
      await page.getByRole('menuitem', { name: '重命名会话', exact: true }).click()
      const dialog = page.getByRole('dialog', { name: '重命名会话', exact: true })
      await dialog.waitFor()
      return dialog
    }
    let dialog = await openRename()
    assert.equal(await dialog.getByRole('textbox').inputValue(), BOARD_SESSION_TITLE)
    await dialog.getByRole('textbox').fill('   ')
    assert.equal(await dialog.getByRole('button', { name: '保存', exact: true }).isDisabled(), true)
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    await dialog.waitFor({ state: 'detached' })
    assert.deepEqual(renames, [], 'a cancelled rename sends nothing')
    const renamed = '登录流程重构 · 已改名'
    dialog = await openRename()
    await dialog.getByRole('textbox').fill(`  ${renamed}  `)
    await dialog.getByRole('button', { name: '保存', exact: true }).click()
    await dialog.waitFor({ state: 'detached' })
    await title.filter({ hasText: renamed }).waitFor()
    assert.deepEqual(renames, [{ status: 200, name: renamed }])
    const stored = await fetch(`${base}/api/sessions`).then((response) => response.json())
    assert.equal(
      stored.sessions.find((session) => session.id === BOARD_SESSION_ID)?.name,
      renamed,
      'the runtime stores the trimmed name',
    )
    await context.close()
  }
  report.checks.push(
    'Session title ▾ › 重命名会话: a blank name cannot be saved and cancelling sends nothing; saving trims the name, PATCHes the real runtime once and updates the header title',
  )

  // 7e. 第 4 期快捷键改绑：⌘J / ⌘I / ⌥↑⌥↓ / 插话 / ⌘/ 都在 shortcut-store，可在“设置 → 快捷键”改绑；
  // 改绑立即生效、旧键失效；⌘/ 速查列出当前绑定，Esc 关闭不停止运行。v2：⌘J 聚焦侧栏会话列表。
  // 最后恢复默认（偏好经 Runtime 同步，免得影响之后的上下文）。
  {
    const { context, aborts } = await sessionContext({
      viewport: { width: 1440, height: 900 },
      colorScheme: 'light',
    })
    const inputs = []
    await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/input$`), async (route) => {
      inputs.push(route.request().postDataJSON())
      await route.fulfill({
        json: { ok: true, inputId: `in-${inputs.length}`, queueRevision: inputs.length },
      })
    })
    const page = await openBoard(context, 'shortcut rebinding')
    const composer = page.locator('.focus-composer textarea').first()
    const inspector = page.locator('[data-session-inspector]')
    const sidebarFocused = () =>
      page.evaluate(() => document.activeElement?.hasAttribute('data-sidebar-row') ?? false)
    const help = page.locator('[data-shortcut-help]')
    const helpRow = (key) => help.locator(`[data-shortcut-help-entry="${key}"]`)
    const keyName = mod === 'Meta' ? '⌘' : 'Ctrl'
    const ariaMod = mod === 'Meta' ? 'Meta' : 'Control'
    const waitForInputs = async (count) => {
      for (let i = 0; i < 30 && inputs.length < count; i += 1) await page.waitForTimeout(100)
    }
    const openRunningSession = async () => {
      await openSession(page)
      await page.locator('[data-run-bar="running"]').waitFor({ timeout: 30_000 })
      await page.locator('[data-shell-sidebar] [data-sidebar-row][aria-current="page"]').waitFor()
    }
    await openRunningSession()
    assert.equal(
      await page
        .locator('[data-shell-sidebar] [data-sidebar-entry="board"]')
        .getAttribute('aria-keyshortcuts'),
      `${ariaMod}+1`,
    )
    // ⌘/ 速查（默认键）：三组，列出 ⌘J；Esc 关闭、焦点回到输入框、不停止运行。
    // 换成矮窗口打开，列表才需要滚动，键盘滚动与焦点框的检查才有意义；关闭后恢复原尺寸。
    // 点进输入框后再按一下 End，让最近一次交互是键盘（边打字边按 ⌘/ 的真实路径）：只用鼠标点过、
    // 再按带修饰键的 ⌘/ 时，Chromium 不把打开时的程序聚焦算作 focus-visible，本来就不描边，
    // 检查不出“打开时压住焦点框”。
    await composer.click()
    await page.keyboard.press('End')
    await page.setViewportSize({ width: 1280, height: 560 })
    await page.keyboard.press(`${mod}+Slash`)
    await help.waitFor({ timeout: 10_000 })
    assert.deepEqual(
      await help
        .locator('[data-shortcut-help-group]')
        .evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-shortcut-help-group'))),
      ['global', 'sidebar', 'board', 'session'],
    )
    // 键盘打开时焦点直接落在可滚动的列表上（不落到“在设置中改绑”上被回车误触）：
    // 列表此时已是 focus-visible，但打开时这一次程序聚焦不描边；第一下翻页键立即滚动列表，
    // 之后恢复焦点框；Shift+Tab 绕回关闭键，再 Tab 回到列表。
    const helpList = help.locator('[data-slot="dialog-body"]')
    const helpListState = () =>
      helpList.evaluate((el) => ({
        focused: el === document.activeElement,
        focusVisible: el.matches(':focus-visible'),
        outline: getComputedStyle(el).outlineStyle,
        scrollable: el.scrollHeight > el.clientHeight,
        scrolled: el.scrollTop > 0,
      }))
    assert.deepEqual(
      await helpListState(),
      { focused: true, focusVisible: true, outline: 'none', scrollable: true, scrolled: false },
      'the help list takes focus on open without a focus ring',
    )
    await page.keyboard.press('PageDown')
    await page.waitForFunction(
      () => document.querySelector('[data-shortcut-help] [data-slot="dialog-body"]')?.scrollTop > 0,
    )
    assert.deepEqual(
      await helpListState(),
      { focused: true, focusVisible: true, outline: 'solid', scrollable: true, scrolled: true },
      'PageDown right after opening scrolls the help list and brings its focus ring back',
    )
    await page.keyboard.press('Shift+Tab')
    assert.equal(
      await page.evaluate(() => {
        const active = document.activeElement
        return active?.closest('[data-shortcut-help]') && active.matches('button')
          ? active.textContent.trim()
          : null
      }),
      '关闭对话框',
      'Shift+Tab from the help list wraps to the close button',
    )
    await page.keyboard.press('Tab')
    assert.equal(
      (await helpListState()).focused,
      true,
      'Tab from the close button returns to the list',
    )
    assert.match((await helpRow('agentPanel').innerText()).replace(/\s+/g, ' '), /J/)
    // 快捷键 v2：⌘1 全部代理、⌘2 功能；会话主页与组件默认未绑定；⌘[ 与 showBoard 不再列出。
    for (const [action, digit] of [
      ['openBoard', '1'],
      ['openMore', '2'],
    ])
      assert.deepEqual(
        await helpRow(action).locator('[data-slot="kbd"]').allTextContents(),
        [mod === 'Meta' ? 'Cmd' : 'Ctrl', digit],
        action,
      )
    for (const action of ['openSession', 'openComponents'])
      assert.equal(await helpRow(action).locator('[data-slot="kbd"]').count(), 0, action)
    for (const retired of ['backToBoard', 'showBoard'])
      assert.equal(await helpRow(retired).count(), 0, retired)
    await page.keyboard.press('Escape')
    await help.waitFor({ state: 'detached' })
    assert.deepEqual(aborts, [], 'Esc in the shortcut help never stops the run')
    assert.equal(
      await page.evaluate(() => document.activeElement?.matches('.focus-composer textarea')),
      true,
    )
    await page.setViewportSize({ width: 1440, height: 900 })

    // 设置 → 快捷键：聚焦侧栏列表 F9（F6 默认属于焦点区域切换）、检查器 F7、插话 ⌘↵。
    await page.goto(`${base}/#/config/shortcuts`)
    const card = page.locator('[data-config-card="shortcuts-bindings"]')
    await card.waitFor({ timeout: 30_000 })
    const record = async (action, keys) => {
      await card.locator(`#shortcut-${action}`).click()
      await page.keyboard.press(keys)
      await page.waitForFunction(
        (id) => document.querySelector(`#shortcut-${id}`)?.getAttribute('aria-pressed') !== 'true',
        action,
      )
    }
    await record('agentPanel', 'F9')
    await record('toggleInspector', 'F7')
    await record('steerMessage', `${mod}+Enter`)

    // 回到会话页：旧键失效、新键生效，标签跟随。
    await openRunningSession()
    await composer.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForTimeout(400)
    assert.equal(await sidebarFocused(), false, 'the old ⌘J no longer focuses the sidebar')
    await page.keyboard.press('F9')
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-sidebar-row'))
    const inspectorOpen = async () => (await inspector.count()) > 0
    const before = await inspectorOpen()
    await composer.click()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.waitForTimeout(300)
    assert.equal(await inspectorOpen(), before, 'the old ⌘I no longer toggles the inspector')
    await page.keyboard.press('F7')
    await page.waitForFunction(
      (open) => !!document.querySelector('[data-session-inspector]') !== open,
      before,
    )
    await page.keyboard.press('F7')
    await page.waitForFunction(
      (open) => !!document.querySelector('[data-session-inspector]') === open,
      before,
    )
    // 规格 v2 第 4.3 节：排队 / 插话的按法写在运行中的占位文字里（工具条提示退役），跟随改绑。
    assert.equal(
      await composer.getAttribute('placeholder'),
      `运行中：Enter 排队，${keyName} + Enter 插话`,
    )
    await composer.fill('改绑后插话')
    await composer.press('Alt+Enter')
    await page.waitForTimeout(500)
    assert.equal(inputs.length, 0, 'the old ⌥↵ no longer steers')
    await composer.fill('改绑后插话')
    await composer.press(`${mod}+Enter`)
    await waitForInputs(1)
    assert.deepEqual(
      inputs.map((input) => input?.behavior),
      ['steer'],
    )
    await page.waitForFunction(
      () => document.querySelector('.focus-composer textarea')?.value === '',
    )
    await composer.fill('排队一条')
    await composer.press('Enter')
    await waitForInputs(2)
    assert.deepEqual(
      inputs.map((input) => input?.behavior),
      ['steer', 'followUp'],
    )
    // 速查显示改绑后的键，并能跳到设置里改绑。
    await composer.click()
    await page.keyboard.press(`${mod}+Slash`)
    await help.waitFor({ timeout: 10_000 })
    assert.match(await helpRow('agentPanel').innerText(), /F9/)
    assert.match(await helpRow('toggleInspector').innerText(), /F7/)
    await help.getByRole('button', { name: '在设置中改绑', exact: true }).click()
    await card.waitFor({ timeout: 30_000 })
    assert.ok(page.url().includes('/config/shortcuts'))
    // 恢复默认：⌘J 重新聚焦侧栏会话列表。
    await card.getByRole('button', { name: '恢复默认', exact: true }).click()
    const confirm = page.getByRole('alertdialog').or(page.getByRole('dialog'))
    await confirm.getByRole('button', { name: '恢复默认', exact: true }).click()
    await page.waitForFunction(
      () => JSON.parse(localStorage.getItem('pisper-shortcuts') || '{}').agentPanel === 'Mod+KeyJ',
    )
    await openRunningSession()
    await composer.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-sidebar-row'))
    assert.deepEqual(aborts, [], 'no Esc in this section stops the run')
    await context.close()
  }
  report.checks.push(
    'Shortcut rebinding: ⌘/ help lists global/board/session bindings, opens with focus on its list without a focus ring (PageDown scrolls it at once and brings the ring back, Shift+Tab wraps to the close button) and Esc closes it without stopping the run; after rebinding the sidebar-list focus to F9, the inspector to F7 and steer to ⌘↵ in Settings → Shortcuts, the old keys stop working, the new ones work at once, the help shows the new keys and links back to Settings; reset restores ⌘J',
  )

  assert.deepEqual(report.pageErrors, [])
  report.status = 'passed'
  console.log(JSON.stringify(report, null, 2))
} catch (error) {
  report.status = 'failed'
  report.failure = String(error?.stack || error)
  if (failurePage && !failurePage.isClosed()) {
    await failurePage.screenshot({ path: join(output, 'failure.png') }).catch(() => {})
    console.log(
      (
        await failurePage
          .locator('body')
          .ariaSnapshot()
          .catch(() => '')
      ).slice(-6000),
    )
  }
  throw error
} finally {
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  await app.close()
  for (const directory of [dataDir, workspace, fixtureHome]) {
    assert.equal(dirname(directory), output, 'Unsafe temporary cleanup path')
    await rm(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
  console.log(`Board UI smoke report and screenshots: ${output}`)
}
