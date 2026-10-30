// 看板与新外壳冒烟：真实生产构建 + 隔离的临时 Runtime；会话列表、/live、审批与重试
// 只在浏览器边界用固定数据提供（scripts/fixtures/board-fixture.mjs），不连接模型、不写真实数据。
// 覆盖第 1 期验收：1440×900 / 1080×680、浅色 / 深色、模拟 Windows 自绘标题栏（右上 144px
// 不被遮挡、无硬裁切、无横向滚动）、泳道与空泳道竖条、内联批准与重试、critical 只开确认层、
// 舰队计量、⌘2/3/4、⌘N、逐个处理、卡片焦点环、未聚焦时动作键无效、今天没有活动时的看板骨架、
// 手机底部标签栏，以及窄化的桌面窗口不进入手机外壳。
// 第 2 期：固定数据默认模拟 board.view / board.events（事件流 + view=board，带 card 摘要），
// 另有一段把能力里的 board.* 去掉，验证旧桌面 Runtime 退回第 1 期的 /api/sessions + /live 路径；
// 再用被测的真实 Runtime 提供 /api/board/events（长连接），核对每个标签页只有 1 条连接、快照与增量、
// 页面隐藏时断开、回到可见后带 after 重连。
// 第 3 期会话页：隔离 Runtime 里写入一条真实会话（转录、/live 来自真实 Runtime），看板与泳道条仍由
// 固定数据提供；覆盖泳道条内联允许 / critical 只开确认层、⌘J 代理面板、⌥↑/⌥↓ 切换、⌘I 检查器、
// ⇧⌘B 回到看板并把焦点还给来源卡片、运行中 Esc 只停止不返回、1080×680 + 自绘标题栏的检查器覆盖层，
// 以及手机 390 的会话页仍可用。
// 第 4 期：快捷键改绑（⌘J / ⌘I / 插话改绑后立即生效、旧键失效）与 ⌘/ 速查（标签按 ⌘1 会话…⌘4 更多编号，
// ⌘[ 已退役）。
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

// critical 卡片的审批详情来自 /live（错峰拉取）；详情到达前“允许…”不可点，先等它就绪。
async function waitForApprovals(page) {
  for (const id of ['fx-critical', 'fx-standard'])
    await page
      .locator(`[data-board-card="${id}"]`)
      .getByRole('button', { name: id === 'fx-critical' ? '允许…' : '允许' })
      .and(page.locator(':enabled'))
      .waitFor({ timeout: 20000 })
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
      .filter(({ element }) => element.closest('[data-app-titlebar], [data-board-root]'))
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
      lanes: [...document.querySelectorAll('[data-board-lane]')].map(
        (lane) => `${lane.getAttribute('data-board-lane')}:${lane.getAttribute('data-lane-mode')}`,
      ),
    }
  })
  report.geometry.push({ label, ...result })
  assert.equal(result.horizontalScroll, false, `${label}: document must not scroll horizontally`)
  assert.deepEqual(result.intrusions, [], `${label}: top-right 144px stays free for window buttons`)
  assert.deepEqual(result.clipped, [], `${label}: no hard-clipped buttons`)
  assert.deepEqual(result.cardOverflow, [], `${label}: card content never overflows its card`)
  return result
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

  // 1. 桌面尺寸 × 主题矩阵：每一档都注入自绘标题栏桥接，检查几何与泳道结构。
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
      assert.equal(await page.locator('[data-app-titlebar]').isVisible(), true)
      assert.equal(await page.locator('[data-custom-titlebar]').count(), 1)
      assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
      const result = await geometry(page, label)
      assert.equal(result.windowControls, 3, `${label}: three window buttons in the title bar`)
      // 第 1 期 Runtime 不声明 sessionQueue：排队泳道整条隐藏，其余三条都有卡片时全部展开。
      assert.deepEqual(result.lanes, ['attention:expanded', 'running:expanded', 'done:expanded'])
      for (const [lane, ids] of Object.entries({
        attention: ['fx-critical', 'fx-standard', 'fx-failed'],
        running: ['fx-running', 'fx-running-agents'],
        done: ['fx-done-unread', 'fx-done-pinned'],
      }))
        for (const id of ids)
          assert.equal(
            await page.locator(`[data-board-card="${id}"]`).getAttribute('data-lane'),
            lane,
            `${label}: ${id} sits in ${lane}`,
          )
      await page.screenshot({ path: join(output, `board-${width}-${colorScheme}.png`) })
      await context.close()
    }
  }
  report.checks.push(
    '1440x900 and 1080x680, light and dark, simulated Windows custom title bar: no horizontal scroll, top-right 144px free of interactive elements, no hard-clipped buttons; lanes attention/running/done expanded, queued hidden without the sessionQueue capability',
  )

  // 2. 1440 浅色上的交互：舰队计量、逐个处理、critical 确认层、standard 内联批准、重试、快捷键。
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'interactions')
    await waitForApprovals(page)
    const fleet = page.locator('[data-app-titlebar]').getByRole('group', {
      name: '代理概况：2 个运行中，3 项需要你',
      exact: true,
    })
    await fleet.waitFor()
    await fleet.getByRole('button', { name: '3 项需要你，查看详情', exact: true }).click()
    const panel = page.locator('[data-slot="popover-content"]')
    await panel.getByText('等待最久', { exact: true }).waitFor()
    assert.equal(await panel.getByRole('button', { name: /允许/ }).count(), 0)
    assert.ok((await panel.getByRole('button', { name: /前往/ }).count()) >= 1)
    await page.keyboard.press('Escape')
    await panel.waitFor({ state: 'hidden' })
    report.checks.push(
      'Fleet meter counts 2 running and 3 needing you; its popover only navigates and has no approve button',
    )

    // T 进入逐个处理（其余泳道变暗，底部提示条），Esc 退出。
    await page.locator('[data-board-card="fx-critical"]').focus()
    await page.keyboard.press('t')
    const triageBar = page.locator('[data-triage-bar]')
    await triageBar.waitFor()
    assert.match(await triageBar.innerText(), /逐个处理 1\/3/)
    await page.keyboard.press('Escape')
    await triageBar.waitFor({ state: 'hidden' })
    report.checks.push(
      'T enters triage with a progress bar over the three attention items; Esc exits',
    )

    // J 从空白处移动焦点：卡片的 2px 焦点环必须真的画出来（--focus 色），不能被 outline-none 吃掉。
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
    assert.notEqual(ring.style, 'none', 'the focused card shows an outline')
    assert.equal(ring.width, '2px')
    assert.equal(ring.color, ring.focus, 'the focus ring uses --focus')
    // 焦点不在任何卡片上时，A / D / R 不能落到“第一张卡片”上。
    await page.evaluate(() => document.activeElement?.blur())
    for (const key of ['r', 'a', 'd']) await page.keyboard.press(key)
    await page.waitForTimeout(500)
    assert.equal(fixture.posts.length, 0, 'action keys without a focused card send nothing')
    assert.equal(await page.getByRole('dialog').count(), 0)
    report.checks.push(
      'J shows a visible 2px --focus ring on the focused card; A / D / R with no card focused send nothing and open nothing',
    )

    // critical：点“允许…”与键盘 A 都只打开确认层，不发出批准请求。
    const critical = page.locator('[data-board-card="fx-critical"]')
    const confirm = page.getByRole('dialog', { name: '确认高风险操作', exact: true })
    await critical.getByRole('button', { name: '允许…' }).click()
    await confirm.waitFor()
    await confirm.getByText('git push --force-with-lease origin fix/ci').first().waitFor()
    assert.equal(fixture.posts.length, 0, 'clicking 允许… must not approve')
    await page.keyboard.press('Escape')
    await confirm.waitFor({ state: 'hidden' })
    await critical.focus()
    await page.keyboard.press('a')
    await confirm.waitFor()
    assert.equal(
      await page.evaluate(() => document.activeElement?.hasAttribute('data-approval-hold')),
      true,
      'the confirmation layer focuses the hold button',
    )
    assert.equal(fixture.posts.length, 0, 'keyboard A on a critical card must not approve')
    await page.keyboard.press('Escape')
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(await critical.getAttribute('data-lane'), 'attention')
    report.checks.push(
      'Critical approval: both the 允许… button and keyboard A only open the confirmation layer; no POST is sent',
    )

    // standard：一键允许发出正确的 POST，卡片换到运行中。
    // 等响应而不是请求：路由回调记录请求体发生在请求发出之后。
    const [approveResponse] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname ===
            '/api/sessions/fx-standard/approvals/approval-standard',
      ),
      page
        .locator('[data-board-card="fx-standard"]')
        .getByRole('button', { name: /^允许( A)?$/ })
        .click(),
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
    report.checks.push(
      'Standard approval 允许 sends POST /api/sessions/fx-standard/approvals/approval-standard {"approved":true} and the card moves to running',
    )

    // 失败卡片重试：POST /retry（SSE），卡片进入运行中。
    await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname === '/api/sessions/fx-failed/retry',
      ),
      page.locator('[data-board-card="fx-failed"]').getByRole('button', { name: /^重试/ }).click(),
    ])
    await page.locator('[data-board-card="fx-failed"][data-lane="running"]').waitFor()
    assert.equal(fixture.posts.at(-1).path, '/api/sessions/fx-failed/retry')
    await page
      .locator('[data-app-titlebar]')
      .getByRole('group', { name: '代理概况：4 个运行中，1 项需要你', exact: true })
      .waitFor()
    report.checks.push(
      'Failed card 重试 posts /api/sessions/fx-failed/retry, moves to running and the fleet meter follows',
    )

    // ⌘N 在看板上聚焦启动台，不打开派发浮层。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press(`${mod}+KeyN`)
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-launch-input'))
    assert.equal(await page.getByRole('dialog').count(), 0)
    report.checks.push('⌘N on the board focuses the launch console instead of opening an overlay')

    // ⌘2 / ⌘3 / ⌘4 切换看板 / 组件 / 更多（⌘1 是“会话”，见 smoke-session-home）；输入框内带修饰键的快捷键同样生效。
    for (const [key, hash, tab] of [
      ['Digit3', '#/components', 'components'],
      ['Digit4', '#/more', 'more'],
      ['Digit2', '#/board', 'board'],
    ]) {
      await page.keyboard.press(`${mod}+${key}`)
      await page.waitForFunction((expected) => location.hash === expected, hash)
      await page
        .locator(`[data-app-titlebar] [data-shell-tab="${tab}"][aria-current="page"]`)
        .waitFor()
    }
    await page.locator('[data-board-root]').waitFor()
    report.checks.push('⌘3 / ⌘4 / ⌘2 switch to components, more and back to the board')
    await context.close()
  }

  // 3. 空泳道竖条：没有需要你的事项时，需要你泳道收成“已清空”竖条；已完成可手动折叠再展开。
  {
    const { context } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      {
        bridge: desktopBridgeScript,
        sessions: boardFixture().sessions.filter((session) => !session.needsAttention),
      },
    )
    const page = await openBoard(context, 'empty lane')
    const attention = page.locator('[data-board-lane="attention"]')
    await attention.and(page.locator('[data-lane-mode="rail"]')).waitFor()
    assert.match(await attention.innerText(), /已清空/)
    const railWidth = (await attention.boundingBox()).width
    assert.ok(Math.abs(railWidth - 56) <= 1, `empty lane rail is 56px wide: ${railWidth}`)
    await page
      .locator('[data-app-titlebar]')
      .getByRole('button', { name: '全部顺畅，查看详情', exact: true })
      .waitFor()
    await page.getByRole('button', { name: '折叠“已完成”', exact: true }).first().click()
    const done = page.locator('[data-board-lane="done"]')
    await done.and(page.locator('[data-lane-mode="rail"]')).waitFor()
    await geometry(page, 'empty lane rails')
    await page.screenshot({ path: join(output, 'board-rails.png') })
    await done.getByRole('button', { name: /展开已折叠的泳道/ }).click()
    await done.and(page.locator('[data-lane-mode="expanded"]')).waitFor()
    report.checks.push(
      'An empty attention lane collapses to a 56px “已清空” rail, the fleet meter reads all clear, and the done lane collapses to a rail and expands again',
    )
    await context.close()
  }

  // 3b. 今天没有活动（只有 2 天前完成的会话）：照常显示看板骨架，已完成泳道保留时间窗切换与历史入口，
  // 不能把有历史的用户提示成“还没有任务”，也不能退回旧首页式的居中输入框。
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
    await page.getByRole('heading', { name: '今天还没有活动', exact: true }).waitFor()
    assert.equal(await page.getByRole('heading', { name: '还没有任务', exact: true }).count(), 0)
    const done = page.locator('[data-board-lane="done"]')
    await done.and(page.locator('[data-lane-mode="expanded"]')).waitFor()
    await page
      .locator('[data-board-lane="attention"][data-lane-mode="rail"]')
      .getByText('已清空')
      .waitFor()
    await done.getByRole('button', { name: /全部历史会话/ }).waitFor()
    await page.locator('[data-attention-capsule]').getByText('没有需要你处理的事项').waitFor()
    await geometry(page, 'quiet board')
    await page.screenshot({ path: join(output, 'board-quiet.png') })
    await done.getByRole('button', { name: '切到 7 天', exact: true }).click()
    await page.locator('[data-board-card="fx-done-unread"][data-lane="done"]').waitFor()
    // 时间窗经 Runtime 同步持久化：恢复默认，避免影响后面的场景。
    await done.getByRole('button', { name: '7 天', exact: true }).click()
    await page.getByRole('menuitemradio', { name: '今天', exact: true }).click()
    await page.getByRole('heading', { name: '今天还没有活动', exact: true }).waitFor()
    report.checks.push(
      'No activity today (only sessions finished 2 days ago): the board skeleton stays (attention rail “已清空”, done lane expanded) with the 7-day switch and 全部历史会话; switching to 7 days shows the older cards',
    )
    await context.close()
  }

  // 4. 手机 App（390×844）：底部标签栏、看板标签的需要你徽标，没有桌面标题栏。
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
    const tabBar = page.getByTestId('mobile-tab-bar')
    await tabBar.waitFor()
    assert.equal(
      await page.evaluate(() => document.documentElement.hasAttribute('data-phone-shell')),
      true,
    )
    assert.equal(await page.locator('[data-app-titlebar]').count(), 0)
    const boardTab = tabBar.locator('[data-shell-tab="board"]')
    await page.waitForFunction(
      () =>
        document
          .querySelector('[data-testid="mobile-tab-bar"] [data-shell-tab="board"]')
          ?.getAttribute('aria-label') === '看板, 3 项需要你',
    )
    assert.equal(await boardTab.getAttribute('aria-current'), 'page')
    assert.equal((await boardTab.innerText()).replace(/\s+/g, ''), '3看板')
    assert.deepEqual(
      await tabBar
        .locator('[data-shell-tab]')
        .evaluateAll((tabs) => tabs.map((tab) => tab.getAttribute('data-shell-tab'))),
      ['session', 'board', 'more', 'settings'],
    )
    for (const box of await tabBar
      .locator('button')
      .evaluateAll((buttons) => buttons.map((button) => button.getBoundingClientRect().toJSON())))
      assert.ok(box.height >= 40 && box.width >= 44, `tab bar touch target ${JSON.stringify(box)}`)
    await page.locator('[data-board-section="attention"]').waitFor()
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      true,
    )
    await page.screenshot({ path: join(output, `phone-${colorScheme}.png`) })
    // 离开看板后，看板标签仍带计数徽标。
    await tabBar.locator('[data-shell-tab="more"]').click()
    await page.waitForFunction(() => location.hash === '#/more')
    assert.equal(await boardTab.getAttribute('aria-label'), '看板, 3 项需要你')
    await context.close()
  }
  report.checks.push(
    'Phone app 390x844 (light/dark): bottom tab bar 会话 · 看板 · 新任务 · 更多 · 设置 with the amber count badge on 看板 across pages, no desktop title bar, no horizontal scroll',
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
      assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
      assert.equal(await page.locator('[data-app-titlebar]').isVisible(), true)
    } else {
      // 对照组：普通浏览器低于手机断点时进入手机外壳，证明上面的门控确实生效。
      assert.equal(phoneShell, true, `${label}: a phone-width browser uses the phone shell`)
      await page.getByTestId('mobile-tab-bar').waitFor()
    }
    await context.close()
  }
  report.checks.push(
    'Desktop windows narrowed to 1000px and 600px (with and without the custom title bar) keep the desktop title bar and never mount the phone shell; a 600px browser does',
  )

  // 6. 第 2 期数据源：Runtime 声明 board.view / board.events 时看板走事件流，卡片显示 card 摘要
  // （运行中尾巴与活动脚注、失败摘要、已完成结果摘要与改动统计、上下文百分比），运行中泳道底部有
  // “再派发一个并行代理”槽位；旧桌面 Runtime（能力里没有 board.*）退回第 1 期路径且没有错误。
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'board feed')
    await page.locator('[data-board-card="fx-running"] [data-card-tail]').waitFor()
    assert.ok(fixture.eventRequests.length >= 1, 'the board opens the event stream')
    assert.equal(fixture.eventRequests[0], null, 'the first connection asks for a snapshot')
    assert.match(
      await page.locator('[data-board-card="fx-running"] [data-card-tail]').innerText(),
      /rejects expired token/,
    )
    assert.equal(
      await page.locator('[data-board-card="fx-running"] [data-tail-fail]').count(),
      1,
      'only the failing tail line is marked red',
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
    assert.match(
      await page.locator('[data-board-card="fx-running"] [data-card-meta]').innerText(),
      /上下文 41%/,
    )
    // 有 card 时不再为审批卡单独取 /live，也不再轮询完整目录之外的 /change-summary。
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
    await page.locator('[data-lane-dispatch]').click()
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-launch-input'))
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
      'approval cards use /live',
    )
    assert.equal(await page.locator('[data-card-tail]').count(), 0, 'no empty tail boxes')
    assert.match(
      await page.locator('[data-board-card="fx-running"] [data-card-meta]').innerText(),
      /6 条消息/,
    )
    await context.close()
  }
  report.checks.push(
    'Board feed: with board.view/board.events the board opens the event stream (snapshot first, then reconnects with after), shows running tails with the failing line in red, failure summaries, done previews with change stats and context %, takes approvals from cards without /live or /change-summary, and the running lane slot focuses the launch console; an old runtime without board.* falls back to /api/sessions + /live with no event-stream or view=board requests and no empty tail boxes',
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
    await page.locator('[data-lane-strip]').waitFor({ timeout: 30_000 })
    await page.waitForTimeout(1_000)
    assert.equal(connections(), 1, 'the session page reuses the board stream')
    await page.keyboard.press(`${mod}+Shift+KeyB`)
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
  // 几何：横向滚动、右上 144px、标题栏 + 泳道条 + 页头的高度，泳道条与检查器里的控件没有越出视口、
  // 也没有被别的元素挡住（取控件中心点命中测试）。
  // 覆盖层打开时盖住对话列右侧（含页头右端）是设计本身，那时只核对泳道条与检查器里的控件。
  function sessionGeometry(page, { header = true } = {}) {
    return page.evaluate((includeHeader) => {
      const rect = (selector) => document.querySelector(selector)?.getBoundingClientRect() ?? null
      const header = rect('[data-session-header]')
      const strip = rect('[data-lane-strip]')
      const overlay = rect('[data-inspector-overlay]')
      const blocked = [
        ...document.querySelectorAll(
          '[data-session-inspector] button, [data-session-inspector] [role="tab"], [data-lane-strip] button' +
            (includeHeader ? ', [data-session-header] button' : ''),
        ),
      ]
        .filter((element) => {
          const box = element.getBoundingClientRect()
          if (!box.width || !box.height || getComputedStyle(element).visibility === 'hidden')
            return false
          if (box.right > innerWidth + 0.5 || box.bottom > innerHeight + 0.5 || box.left < -0.5)
            return true
          // 芯片行可以横向滚动；只核对行内可见的控件。
          const row = element.closest('[data-strip-row]')?.getBoundingClientRect()
          if (row && (box.left < row.left - 0.5 || box.right > row.right + 0.5)) return false
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
        stripHeight: strip ? Math.round(strip.height) : null,
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

  // 7a. 1440×900：泳道条、确认层、内联允许、⌘J、⌘I、⌥↑/⌥↓、Esc、⇧⌘B（⌘[ 已退役，不再离开会话页）。
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
    // 从卡片打开（点标题区域，避开卡片上的按钮），之后 ⇧⌘B 回到看板要把焦点还给这张卡片。
    const openedAt = Date.now()
    await realCard.click({ position: { x: 60, y: 14 } })
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    const strip = page.locator('[data-lane-strip]')
    await strip.waitFor({ timeout: 30_000 })
    await page.locator('[data-run-bar="running"]').waitFor({ timeout: 30_000 })
    await page.locator(`[data-strip-chip="${BOARD_SESSION_ID}"][data-current]`).waitFor()
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
    assert.equal(geo.stripHeight, 52, `session page: 52px lane strip ${JSON.stringify(geo)}`)
    assert.equal(geo.headerHeight, 44, 'session page: 44px session header')
    assert.equal(geo.workspace, 'aside', 'wide session page docks the inspector')
    assert.deepEqual(geo.intrusions, [], 'session page: top-right 144px stays free')
    assert.equal(await page.locator('[data-session-title]').innerText(), BOARD_SESSION_TITLE)
    await page.screenshot({ path: join(output, 'session-1440.png') })

    // critical 芯片只有[查看]：打开看板同一个确认层，Esc 关闭它，不批准、也不停止运行。
    const criticalChip = page.locator('[data-strip-chip="fx-critical"]')
    await criticalChip.locator('[data-strip-review]').waitFor({ timeout: 20_000 })
    assert.equal(await criticalChip.locator('[data-strip-approve]').count(), 0)
    await criticalChip.locator('[data-strip-review]').click()
    const confirm = page.getByRole('dialog', { name: '确认高风险操作', exact: true })
    await confirm.waitFor()
    await confirm.getByText('git push --force-with-lease origin fix/ci').first().waitFor()
    await page.keyboard.press('Escape')
    await confirm.waitFor({ state: 'hidden' })
    assert.equal(fixture.posts.length, 0, 'the strip review button never approves')
    assert.deepEqual(aborts, [], 'Esc on the confirmation layer does not stop the run')
    assert.equal(page.url().endsWith(`#/board/${BOARD_SESSION_ID}`), true)

    // standard 芯片内联允许：一次 POST，请求体与看板一致。
    const [approveResponse] = await Promise.all([
      page.waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          new URL(response.url()).pathname ===
            '/api/sessions/fx-standard/approvals/approval-standard',
      ),
      page.locator('[data-strip-chip="fx-standard"] [data-strip-approve]').click(),
    ])
    assert.equal(approveResponse.request().postData(), JSON.stringify({ approved: true }))
    assert.deepEqual(fixture.posts, [
      {
        method: 'POST',
        path: '/api/sessions/fx-standard/approvals/approval-standard',
        body: { approved: true },
      },
    ])
    report.checks.push(
      'Session page lane strip: the critical chip only offers 查看, which opens the shared confirmation layer (Esc closes it without approving or stopping); the standard chip approves inline with one POST {"approved":true}',
    )

    // ⌘J：左侧代理面板按泳道分组，critical 行只有查看；Esc 关闭并把焦点还回输入框，不停止运行。
    const prompt = page.locator('.focus-composer textarea')
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    const panel = page.locator('[data-agent-panel]')
    await panel.waitFor()
    const lanes = await panel
      .locator('[data-agent-lane]')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-agent-lane')))
    assert.equal(lanes[0], 'attention', `agent panel groups by lane: ${lanes}`)
    assert.ok(lanes.includes('running'), `agent panel lists running agents: ${lanes}`)
    assert.equal(
      await panel.locator('[data-agent-row="fx-critical"] [data-strip-approve]').count(),
      0,
    )
    assert.equal(
      await panel.locator('[data-agent-row="fx-critical"] [data-strip-review]').count(),
      1,
    )
    const box = await panel.boundingBox()
    assert.ok(
      Math.round(box.width) === 320 && box.x <= 1,
      `agent panel geometry ${JSON.stringify(box)}`,
    )
    await page.keyboard.press('Escape')
    await panel.waitFor({ state: 'detached' })
    await page.waitForFunction(() => document.activeElement?.matches('.focus-composer textarea'))
    assert.deepEqual(aborts, [], 'Esc on the agent panel does not stop the run')
    report.checks.push(
      '⌘J opens the 320px agent panel grouped by lane (critical rows only offer 查看); Esc closes it, returns focus to the composer and does not stop the run',
    )

    // ⌘I：输入框内也能开合检查器。
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-session-inspector]').waitFor({ state: 'detached' })
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-session-inspector]').waitFor()
    report.checks.push('⌘I closes and reopens the docked inspector from inside the composer')

    // ⌥↓ / ⌥↑：按泳道条顺序换代理，只换路由（草稿按会话保留），浏览器后退回到看板。
    await prompt.fill('会话页的未发送草稿')
    await page.keyboard.press('Alt+ArrowDown')
    await page.waitForFunction((id) => !location.hash.endsWith(id), BOARD_SESSION_ID)
    const next = await page.evaluate(() => location.hash)
    assert.match(next, /^#\/board\/fx-/, 'Alt+ArrowDown opens the next agent on the strip')
    await page.locator('[data-lane-strip]').waitFor()
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
    await page.locator(`[data-strip-chip="${BOARD_SESSION_ID}"][data-current]`).waitFor()
    assert.equal(
      await page.evaluate(() => location.hash),
      `#/board/${BOARD_SESSION_ID}`,
      'back-to-back ⌥↓ ⌥↑ returns to the starting session',
    )
    report.checks.push(
      '⌥↓ / ⌥↑ switch agents along the strip by route and keep each session draft; back-to-back presses start from the switch that has not committed yet',
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
    // ⇧⌘B：打开看板，焦点落在打开它的卡片上。
    await page.keyboard.press(`${mod}+Shift+KeyB`)
    await page.locator('[data-board-root]').waitFor()
    await page.waitForFunction(
      (id) =>
        document.activeElement?.closest('[data-board-card]')?.getAttribute('data-board-card') ===
        id,
      BOARD_SESSION_ID,
      { timeout: 10_000 },
    )
    report.checks.push(
      '⌘[ no longer navigates; ⇧⌘B opens the board and focuses the card the session was opened from',
    )
    await context.close()
  }

  // 7b. 1080×680 + 模拟 Windows 自绘标题栏：泳道条 36px 单行，检查器是覆盖层（默认关闭），
  // 打开后在视口内、控件无裁切无遮挡、无横向滚动；Esc 先关覆盖层，不停止运行也不离开。
  {
    const { context, aborts } = await sessionContext(
      { viewport: { width: 1080, height: 680 }, colorScheme: 'light' },
      { bridge: desktopBridgeScript },
    )
    const page = await openBoard(context, 'session 1080')
    await openSession(page)
    await page.locator('[data-lane-strip]').waitFor()
    await page.locator('[data-run-bar="running"]').waitFor({ timeout: 30_000 })
    await settle(page)
    let geo = await sessionGeometry(page)
    assert.equal(geo.stripHeight, 36, `1080: single-line strip ${JSON.stringify(geo)}`)
    assert.ok(geo.headerBottom !== null && geo.headerBottom <= 128, `1080: chrome ≤ 128px`)
    assert.equal(geo.workspace, 'overlay')
    assert.equal(await page.locator('[data-session-inspector]').count(), 0, 'overlay starts closed')
    assert.equal(geo.horizontalScroll, false)
    assert.deepEqual(geo.intrusions, [], '1080: top-right 144px stays free')
    assert.deepEqual(geo.blocked, [], '1080: no clipped or covered controls')
    // 在输入框里 ⌘I 打开覆盖层（不抢焦点）、Esc 关闭：焦点留在输入框，不被挪到页头开关，也不停止运行。
    const composerInput = page.locator('.focus-composer textarea').first()
    await composerInput.click()
    await page.keyboard.press(`${mod}+KeyI`)
    await page.locator('[data-inspector-overlay]').waitFor()
    await page.keyboard.press('Escape')
    await page.locator('[data-inspector-overlay]').waitFor({ state: 'detached' })
    await page.waitForTimeout(100)
    assert.equal(
      await page.evaluate(() => document.activeElement?.matches('.focus-composer textarea')),
      true,
      'closing the overlay with Esc keeps focus in the composer',
    )
    assert.deepEqual(aborts, [], 'Esc closes the overlay and does not stop the run')
    await page.locator('[data-inspector-toggle]').click()
    await page.locator('[data-inspector-overlay]').waitFor()
    await page.locator('[data-session-inspector]').waitFor()
    await settle(page)
    // 覆盖层从页头下方开始，并给对话内容让出宽度：停止键、发送键与页头按钮都能点到。
    const reachable = await page.evaluate(() =>
      [
        '[data-run-stop]',
        '.focus-composer .send-button',
        '[data-inspector-toggle]',
        '[data-session-header] [id$="-more"]',
      ].map((selector) => {
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
      `1080 overlay leaves stop, send and header controls reachable ${JSON.stringify(reachable)}`,
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
    await page.locator('[data-session-header]').click({ position: { x: 300, y: 20 } })
    await page.keyboard.press('Escape')
    await page.locator('[data-inspector-overlay]').waitFor({ state: 'detached' })
    await page.waitForTimeout(300)
    assert.deepEqual(aborts, [], 'Esc closes the overlay first and does not stop the run')
    assert.equal(await page.evaluate(() => location.hash), `#/board/${BOARD_SESSION_ID}`)
    await context.close()
  }
  report.checks.push(
    '1080x680 with the simulated custom title bar: 36px single-line strip, chrome ≤ 128px, the inspector is a 420px overlay without a scrim below the session header, the conversation makes room so stop, send and header controls stay reachable, nothing clipped, covered or in the top-right 144px, no horizontal scroll; ⌘I/Esc from the composer keeps focus there, and Esc closes the overlay without stopping the run',
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
    const back = page.getByRole('button', { name: /^打开看板/ })
    const backBox = await back.boundingBox()
    assert.ok(backBox && backBox.height >= 44, `phone back target ${JSON.stringify(backBox)}`)
    await page.screenshot({ path: join(output, 'session-phone.png') })
    await back.click()
    await page.locator('[data-board-root]').waitFor()
    await context.close()
  }
  report.checks.push(
    'Phone app 390x844: the phone session page (no lane strip, no bottom tab bar, composer usable, 44px open-board entry, no horizontal scroll)',
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
      await page.locator('[data-session-header] [aria-label="项目与任务设置"]').click()
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
    'Session header ⋯ › 重命名会话: a blank name cannot be saved and cancelling sends nothing; saving trims the name, PATCHes the real runtime once and updates the header title',
  )

  // 7e. 第 4 期快捷键改绑：⌘J / ⌘I / ⌥↑⌥↓ / 插话 / ⌘/ 都在 shortcut-store，可在“设置 → 快捷键”改绑；
  // 改绑立即生效、旧键失效，标签与 aria-keyshortcuts 跟随；⌘/ 速查列出当前绑定，Esc 关闭不停止运行。
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
    const panel = page.locator('[data-agent-panel]')
    const inspector = page.locator('[data-session-inspector]')
    const agentsButton = page.locator('[data-strip-agents]')
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
      await page.locator('[data-lane-strip]').waitFor()
    }
    await openRunningSession()
    assert.equal(await agentsButton.getAttribute('aria-keyshortcuts'), `${ariaMod}+J`)
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
      ['global', 'board', 'session'],
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
    // 一级标签按标题栏顺序编号：会话 1、看板 2、组件 3、更多 4；⌘[ 的“返回看板”不再列出。
    for (const [action, digit] of [
      ['openSession', '1'],
      ['openBoard', '2'],
      ['openComponents', '3'],
      ['openMore', '4'],
    ])
      assert.deepEqual(
        await helpRow(action).locator('[data-slot="kbd"]').allTextContents(),
        [mod === 'Meta' ? 'Cmd' : 'Ctrl', digit],
        action,
      )
    assert.equal(await helpRow('backToBoard').count(), 0)
    await page.keyboard.press('Escape')
    await help.waitFor({ state: 'detached' })
    assert.deepEqual(aborts, [], 'Esc in the shortcut help never stops the run')
    assert.equal(
      await page.evaluate(() => document.activeElement?.matches('.focus-composer textarea')),
      true,
    )
    await page.setViewportSize({ width: 1440, height: 900 })

    // 设置 → 快捷键：代理面板 F6、检查器 F7、插话 ⌘↵。
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
    await record('agentPanel', 'F6')
    await record('toggleInspector', 'F7')
    await record('steerMessage', `${mod}+Enter`)

    // 回到会话页：旧键失效、新键生效，标签跟随。
    await openRunningSession()
    assert.equal(await agentsButton.getAttribute('aria-label'), '全部代理（F6）')
    assert.equal(await agentsButton.getAttribute('aria-keyshortcuts'), 'F6')
    await composer.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForTimeout(400)
    assert.equal(await panel.count(), 0, 'the old ⌘J no longer opens the agent panel')
    await page.keyboard.press('F6')
    await panel.waitFor({ timeout: 5000 })
    await page.keyboard.press('Escape')
    await panel.waitFor({ state: 'detached' })
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
    assert.equal(
      (await page.locator('[data-composer-hint]').innerText()).trim(),
      `运行中 Enter 排队 · ${keyName} + Enter 插话`,
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
    assert.match(await helpRow('agentPanel').innerText(), /F6/)
    assert.match(await helpRow('toggleInspector').innerText(), /F7/)
    await help.getByRole('button', { name: '在设置中改绑', exact: true }).click()
    await card.waitFor({ timeout: 30_000 })
    assert.ok(page.url().includes('/config/shortcuts'))
    // 恢复默认：⌘J 重新打开代理面板。
    await card.getByRole('button', { name: '恢复默认', exact: true }).click()
    const confirm = page.getByRole('alertdialog').or(page.getByRole('dialog'))
    await confirm.getByRole('button', { name: '恢复默认', exact: true }).click()
    await page.waitForFunction(
      () => JSON.parse(localStorage.getItem('pisper-shortcuts') || '{}').agentPanel === 'Mod+KeyJ',
    )
    await openRunningSession()
    await composer.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await panel.waitFor({ timeout: 5000 })
    await page.keyboard.press('Escape')
    assert.deepEqual(aborts, [], 'no Esc in this section stops the run')
    await context.close()
  }
  report.checks.push(
    'Shortcut rebinding: ⌘/ help lists global/board/session bindings, opens with focus on its list without a focus ring (PageDown scrolls it at once and brings the ring back, Shift+Tab wraps to the close button) and Esc closes it without stopping the run; after rebinding the agent panel to F6, the inspector to F7 and steer to ⌘↵ in Settings → Shortcuts, the old keys stop working, the new ones work at once, labels and aria-keyshortcuts follow, the help shows the new keys and links back to Settings; reset restores ⌘J',
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
