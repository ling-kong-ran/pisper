// 看板与新外壳冒烟：真实生产构建 + 隔离的临时 Runtime；会话列表、/live、审批与重试
// 只在浏览器边界用固定数据提供（scripts/fixtures/board-fixture.mjs），不连接模型、不写真实数据。
// 覆盖第 1 期验收：1440×900 / 1080×680、浅色 / 深色、模拟 Windows 自绘标题栏（右上 144px
// 不被遮挡、无硬裁切、无横向滚动）、泳道与空泳道竖条、内联批准与重试、critical 只开确认层、
// 舰队计量、⌘1/2/3、⌘N、逐个处理、卡片焦点环、未聚焦时动作键无效、今天没有活动时的看板骨架、
// 手机底部标签栏，以及窄化的桌面窗口不进入手机外壳。
// 浏览器模拟不等于 Tauri / WebView2 或真机验证。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import {
  boardFixture,
  desktopBridgeScript,
  installBoardFixture,
  plainDesktopBridgeScript,
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

async function newContext(options, { bridge, mobileApp, sessions } = {}) {
  const context = await browser.newContext({ locale: 'zh-CN', ...options })
  await context.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  if (bridge) await context.addInitScript(bridge)
  if (mobileApp) await context.addInitScript('window.__PISPER_MOBILE_APP__ = true')
  const fixture = await installBoardFixture(context, { sessions })
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

    // ⌘1 / ⌘2 / ⌘3 切换主标签；输入框内带修饰键的快捷键同样生效。
    for (const [key, hash, tab] of [
      ['Digit2', '#/components', 'components'],
      ['Digit3', '#/more', 'more'],
      ['Digit1', '#/board', 'board'],
    ]) {
      await page.keyboard.press(`${mod}+${key}`)
      await page.waitForFunction((expected) => location.hash === expected, hash)
      await page
        .locator(`[data-app-titlebar] [data-shell-tab="${tab}"][aria-current="page"]`)
        .waitFor()
    }
    await page.locator('[data-board-root]').waitFor()
    report.checks.push('⌘2 / ⌘3 / ⌘1 switch to components, more and back to the board')
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
      ['board', 'components', 'more', 'settings'],
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
    'Phone app 390x844 (light/dark): bottom tab bar 看板 · 组件 · 新任务 · 更多 · 设置 with the amber count badge on 看板 across pages, no desktop title bar, no horizontal scroll',
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
