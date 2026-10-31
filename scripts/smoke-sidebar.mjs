// 侧栏冒烟（规格 v2 第 2.1、2.2、2.3、2.8、4.6、9.2 节，A1 验收）：真实生产构建 + 隔离的临时 Runtime；
// 会话列表、/live 与审批只在浏览器边界用固定数据提供（scripts/fixtures/board-fixture.mjs），
// 会话页的那条会话写进本次的临时数据目录，不连接模型、不写真实数据。
// 覆盖：1440×900 / 1080×680 × 浅色 / 深色 × 展开 / 收起，模拟 Windows 自绘标题栏时右上 144px 没有可交互元素、
// 没有横向滚动；窄轨计数（为 0 时变灰不消失）；需要你行的读屏名称与新增需要你的 polite 播报（收起时也播）；
// 快捷处理（悬停 / 聚焦后普通批准一步允许或拒绝，只发一次请求、行收起）；⌥A 聚焦等待最久的行 → A 批准；
// ⌥↑ / ⌥↓ 按侧栏顺序 replace 切换；⌘B；⌘J（收起时临时展开）；F6 在侧栏、顶栏、对话、输入框之间切换；
// 从全部代理 / 设置返回时焦点回到原会话行；1x 截图里运行中的弧与完成逐像素可区分（排队见 smoke-replica-shell）。
// A3（规格 2.3、2.6）：侧栏随页面变化——全部代理页收成状态 / 项目筛选（不列会话行，⌘J / ⌥A 改落到筛选与主区行），
// 设置整页换成“← 返回 Esc”+ 设置目录 + 底部状态行（收起时也是整块 268），Esc 与“← 返回”回到进入设置前的页面，
// 输入框里的 Esc 不离开；远程访问等 /config 功能页与插件页归“功能”（页头“‹ 功能”，不用设置外壳）。
// 浏览器模拟不等于 Tauri / WebView2 或真机验证。
import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import {
  BOARD_SESSION_ID,
  boardFixture,
  desktopBridgeScript,
  installBoardFixture,
  writeBoardSessionFixture,
} from './fixtures/board-fixture.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = await mkdtemp(join(tmpdir(), 'pisper-sidebar-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
const fixtureHome = join(output, 'home')
await mkdir(workspace)
await mkdir(fixtureHome)
// 所有 CLI 扫描限于空的隔离 home；侧栏与看板偏好经 Runtime 同步，也只写进这个临时目录。
process.env.HOME = fixtureHome
process.env.USERPROFILE = fixtureHome
process.env.CODEX_HOME = join(fixtureHome, '.codex')
process.env.CLAUDE_CONFIG_DIR = join(fixtureHome, '.claude')
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace
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
const mod = process.platform === 'darwin' ? 'Meta' : 'Control'
const report = { status: 'running', checks: [], geometry: [], glyphs: [], pageErrors: [] }
let browser
let failurePage

// 会话页用的真实会话放进“运行中”。排队分组要等 Runtime 声明 sessionQueue（A9），这里没有排队行；
// 排队虚线圈与运行中弧 / 完成勾的 1x 对比在 smoke-replica-shell 的图形图例里核对。
const realList = await fetch(`${base}/api/sessions`).then((response) => response.json())
const realItem = realList.sessions.find((session) => session.id === BOARD_SESSION_ID)
assert.ok(realItem, 'the isolated runtime lists the session-page fixture')
const baseSessions = () => [
  ...boardFixture().sessions,
  { ...realItem, pinned: false, unread: false, needsAttention: false, streaming: true },
]
const startedAt = new Date(Date.now() - 12_000).toISOString()

async function newContext(options, { sessions = baseSessions(), prepare } = {}) {
  const context = await browser.newContext({
    locale: 'zh-CN',
    deviceScaleFactor: 1,
    ...options,
  })
  await context.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  await context.addInitScript(desktopBridgeScript)
  const fixture = await installBoardFixture(context, { sessions })
  prepare?.(fixture)
  await context.route('**/api/app-update*', (route) =>
    route.fulfill({ json: { state: 'current', checkedAt: Date.now() } }),
  )
  const aborts = []
  await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/live`), async (route) => {
    const response = await route.fetch()
    const live = await response.json().catch(() => ({}))
    await route.fulfill({
      response,
      json: { ...live, streaming: true, startedAt, lastActivityAt: new Date().toISOString() },
    })
  })
  await context.route(/\/api\/sessions\/[^/]+\/abort$/, async (route) => {
    aborts.push(route.request().url())
    await route.fulfill({ json: { ok: true } })
  })
  return { context, fixture, aborts }
}

async function openSession(context, label) {
  const page = await context.newPage()
  failurePage = page
  page.on('pageerror', (error) => report.pageErrors.push(`${label}: ${error.message}`))
  await page.goto(`${base}/#/board/${BOARD_SESSION_ID}`)
  await page.locator('.focus-composer textarea').first().waitFor({ timeout: 60_000 })
  await page
    .locator(`[data-sidebar-row="${BOARD_SESSION_ID}"][aria-current="page"]`)
    .waitFor({ timeout: 30_000 })
  return page
}

async function settle(page) {
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await page.waitForTimeout(300)
}

const activeRow = (page) =>
  page.evaluate(() => document.activeElement?.getAttribute('data-sidebar-row') ?? null)
const activeRegion = (page) =>
  page.evaluate(
    () =>
      document.activeElement?.closest('[data-focus-region]')?.getAttribute('data-focus-region') ??
      null,
  )

// 侧栏收起偏好经 Runtime 的偏好镜像跨上下文同步：每个上下文结束前确保回到展开。
async function ensureExpanded(page) {
  if ((await page.locator('[data-shell-sidebar="rail"]').count()) === 0) return
  await page.locator('[data-sidebar-toggle="expand"]').click()
  await page.locator('[data-shell-sidebar="expanded"]').waitFor()
  await page.waitForFunction(() =>
    localStorage.getItem('pisper-sidebar')?.includes('"collapsed":false'),
  )
}

// 看板筛选偏好同样跨上下文同步：用过窄轨计数的筛选后清掉。
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

async function geometry(page, label) {
  const result = await page.evaluate(() => {
    const reservedLeft = innerWidth - 144
    const intrusions = [
      ...document.querySelectorAll(
        'button, a[href], input, textarea, select, [role="button"], [role="tab"], [tabindex="0"]',
      ),
    ]
      .filter((element) => !element.closest('.desktop-window-controls'))
      .filter((element) => {
        const box = element.getBoundingClientRect()
        return box.width > 0 && box.height > 0 && box.top < 48 && box.right > reservedLeft + 0.5
      })
      .map((element) => (element.getAttribute('aria-label') || element.textContent || '').trim())
    // 侧栏里的入口与行没有越出侧栏，也没有被别的元素挡住（取中心点命中测试）。
    const sidebar = document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect()
    const covered = [
      ...document.querySelectorAll(
        '[data-shell-sidebar] [data-sidebar-entry], [data-shell-sidebar] [data-sidebar-row], [data-shell-sidebar] [data-rail-count], [data-shell-sidebar] [data-sidebar-filter], [data-shell-sidebar] [data-settings-section], [data-shell-sidebar] [data-settings-back], [data-shell-sidebar] [data-sidebar-fleet-line]',
      ),
    ]
      .filter((element) => {
        const box = element.getBoundingClientRect()
        if (!box.width || !box.height || box.bottom > innerHeight) return false
        if (box.left < sidebar.left - 0.5 || box.right > sidebar.right + 0.5) return true
        const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2)
        return Boolean(hit && !element.contains(hit) && !hit.contains(element))
      })
      .map((element) => element.getAttribute('data-sidebar-row') ?? element.outerHTML.slice(0, 60))
    return {
      width: innerWidth,
      sidebar: Math.round(sidebar?.width ?? 0),
      horizontalScroll:
        document.documentElement.scrollWidth > innerWidth || document.body.scrollWidth > innerWidth,
      intrusions,
      covered,
      windowControls: document.querySelectorAll('.desktop-window-controls button').length,
      customTitlebar: document.querySelectorAll('[data-custom-titlebar]').length,
    }
  })
  report.geometry.push({ label, ...result })
  assert.equal(result.horizontalScroll, false, `${label}: no horizontal scroll`)
  assert.deepEqual(result.intrusions, [], `${label}: top-right 144px stays free`)
  assert.deepEqual(result.covered, [], `${label}: sidebar controls are not covered or clipped`)
  assert.equal(result.windowControls, 3, `${label}: three window buttons`)
  assert.equal(result.customTitlebar, 1, `${label}: custom title bar flag`)
  return result
}

// 1x 截图逐像素：取字形截图（动画停在起点），以左上角像素为底色，统计与底色的最大差与形状掩码。
async function glyphPixels(page, locator) {
  const png = await locator.screenshot({ animations: 'disabled' })
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
    return { width: image.width, height: image.height, maxDiff: Math.round(maxDiff), mask }
  }, png.toString('base64'))
}

function jaccard(a, b) {
  let both = 0
  let either = 0
  for (let index = 0; index < Math.min(a.length, b.length); index += 1) {
    if (a[index] && b[index]) both += 1
    if (a[index] || b[index]) either += 1
  }
  return either ? both / either : 1
}

try {
  browser = await chromium.launch({
    headless: true,
    ...((process.env.PISPER_UI_BROWSER_PATH ?? process.env.PISPER_SMOKE_BROWSER)
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH ?? process.env.PISPER_SMOKE_BROWSER }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })

  // 1. 尺寸 × 主题 × 展开 / 收起：几何、截图与 1x 状态图形。
  for (const [width, height] of [
    [1440, 900],
    [1080, 680],
  ]) {
    for (const colorScheme of ['light', 'dark']) {
      const label = `${width}x${height} ${colorScheme}`
      const { context } = await newContext({ viewport: { width, height }, colorScheme })
      const page = await openSession(context, label)
      await page.waitForFunction(
        (theme) => document.documentElement.dataset.theme === theme,
        colorScheme,
      )
      await page
        .locator('[data-sidebar-group="done"] [data-sidebar-row="fx-done-unread"]')
        .waitFor()
      await settle(page)
      const expanded = await geometry(page, `${label} expanded`)
      assert.equal(expanded.sidebar, 268, `${label}: the sidebar stays expanded at 268px`)
      await page.screenshot({ path: join(output, `sidebar-${width}-${colorScheme}-expanded.png`) })

      // 1x 状态图形：运行中（非当前行）与今天完成。
      const glyph = (group, kind) =>
        page
          .locator(
            `[data-sidebar-group="${group}"] [data-sidebar-row]:not([aria-current]) [data-status-glyph="${kind}"]`,
          )
          .first()
      const run = await glyphPixels(page, glyph('running', 'run'))
      const done = await glyphPixels(page, glyph('done', 'done'))
      const overlap = jaccard(run.mask, done.mask)
      report.glyphs.push({
        label,
        run: run.maxDiff,
        done: done.maxDiff,
        runDoneOverlap: Number(overlap.toFixed(2)),
      })
      assert.equal(run.width, 16, `${label}: glyphs render at 16px in a 1x screenshot`)
      assert.ok(
        run.maxDiff >= done.maxDiff + 40,
        `${label}: the ink running arc stands out from the done tick ${JSON.stringify(report.glyphs.at(-1))}`,
      )
      assert.ok(overlap < 0.5, `${label}: the running arc and the done tick differ in shape`)

      // 收起：60px 窄轨，四个入口 + 两个计数 + 设置，右上 144px 仍空着。
      await page.locator('.focus-composer textarea').first().click()
      await page.keyboard.press(`${mod}+KeyB`)
      await page.locator('[data-shell-sidebar="rail"]').waitFor()
      await page.waitForFunction(
        () => document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect().width === 60,
      )
      await settle(page)
      const rail = await geometry(page, `${label} collapsed`)
      assert.equal(rail.sidebar, 60)
      assert.equal(
        await page.locator('[data-shell-sidebar] [data-sidebar-entry]').count(),
        5,
        `${label}: the rail keeps new task, search, all agents, features and settings`,
      )
      assert.equal(await page.locator('[data-rail-count="attention"]').innerText(), '3')
      assert.equal(await page.locator('[data-rail-count="running"]').innerText(), '3')
      await page.screenshot({ path: join(output, `sidebar-${width}-${colorScheme}-rail.png`) })
      await page.keyboard.press(`${mod}+KeyB`)
      await page.locator('[data-shell-sidebar="expanded"]').waitFor()
      await ensureExpanded(page)
      await context.close()
    }
  }
  report.checks.push(
    '1440x900 and 1080x680, light and dark, simulated Windows custom title bar: the sidebar is 268px expanded and 60px collapsed (⌘B both ways), no horizontal scroll, nothing interactive in the top-right 144px, sidebar entries / rows / rail counts neither clipped nor covered',
    '1x screenshots of real sidebar rows: the 16px running arc (ink) has ≥40 more luminance contrast than the done tick and a different shape (mask overlap < 0.5), in light and dark (the queued ring is compared in smoke-replica-shell until sessionQueue ships)',
  )

  // 2. 读屏名称、快捷处理（悬停 → 允许一次）、新增需要你的 polite 播报（展开与收起都播）、窄轨计数筛选。
  {
    const { context, fixture, aborts } = await newContext({
      viewport: { width: 1440, height: 900 },
      colorScheme: 'light',
    })
    const page = await openSession(context, 'quick actions')
    const row = (id) => page.locator(`[data-sidebar-row="${id}"]`)
    const item = (id) => page.locator(`[data-sidebar-item="${id}"]`)
    assert.match(
      await row('fx-standard').getAttribute('aria-label'),
      /^补充登录模块的单元测试，等你批准，已等 /,
    )
    assert.match(
      await row('fx-critical').getAttribute('aria-label'),
      /^修复 CI 失败，高风险批准，已等 /,
    )
    assert.match(
      await row('fx-failed').getAttribute('aria-label'),
      /^迁移支付回调到队列，运行失败，已等 /,
    )
    // 审批详情就绪后普通批准才给一步允许。
    await row('fx-standard').hover()
    const approve = item('fx-standard').getByRole('button', { name: /^允许/ })
    await approve.waitFor({ timeout: 20_000 })
    await row('fx-critical').hover()
    await item('fx-critical').getByRole('button', { name: /^打开/ }).waitFor()
    assert.equal(await item('fx-critical').getByRole('button', { name: /^允许/ }).count(), 0)
    await row('fx-standard').hover()
    await approve.waitFor()
    // 快捷处理叠在行右侧：标题与原因让出它的宽度（按省略号截断），不从按钮底下穿过去；
    // 指针停在按钮上时整行仍是悬停底色。
    await approve.hover()
    await page.waitForTimeout(300)
    const overlap = await page.evaluate(() => {
      const item = document.querySelector('[data-sidebar-item="fx-standard"]')
      const quick = item.querySelector('[data-sidebar-quick]').getBoundingClientRect()
      const texts = [...item.querySelectorAll('[data-sidebar-row] .truncate')].map((element) =>
        Math.round(element.getBoundingClientRect().right),
      )
      return {
        quickLeft: Math.round(quick.left),
        texts,
        background: getComputedStyle(item.querySelector('[data-sidebar-row]')).backgroundColor,
      }
    })
    assert.ok(overlap.texts.length >= 2, `title and reason measured ${JSON.stringify(overlap)}`)
    for (const right of overlap.texts)
      assert.ok(
        right <= overlap.quickLeft,
        `row text stops before the quick actions ${JSON.stringify(overlap)}`,
      )
    assert.notEqual(
      overlap.background,
      'rgba(0, 0, 0, 0)',
      'the row keeps its hover background under the pointer',
    )
    await approve.click()
    await page.waitForFunction(
      () =>
        !document.querySelector(
          '[data-sidebar-group="attention"] [data-sidebar-item="fx-standard"]:not([hidden])',
        ),
    )
    assert.deepEqual(
      fixture.posts.map((post) => [post.path, post.body]),
      [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
      'one click sends exactly one approval',
    )
    assert.equal(page.url().endsWith(`#/board/${BOARD_SESSION_ID}`), true)
    report.checks.push(
      'Needs-you rows read “title，reason，waited …”; hovering the standard approval reveals 允许 (critical only 打开) while the title and reason truncate before the quick actions and the row keeps its hover background under the buttons; one click sends exactly one {"approved":true} and the row collapses out of 需要你 while the current session stays open',
    )

    // 新增需要你：polite 播报；侧栏收起（分组不挂载）时同样播报。
    const announce = page.locator('[data-sidebar-announce]')
    assert.equal(await announce.getAttribute('aria-live'), 'polite')
    fixture.update('fx-running', {
      streaming: false,
      needsAttention: true,
      attentionReason: 'approval',
    })
    await page.waitForFunction(
      () =>
        document.querySelector('[data-sidebar-announce]')?.textContent ===
        '重构登录流程 需要你：等你批准',
      null,
      { timeout: 15_000 },
    )
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="rail"]').waitFor()
    fixture.update('fx-running-agents', {
      streaming: false,
      needsAttention: true,
      attentionReason: 'failure',
      lastCompletedAt: new Date().toISOString(),
      unread: true,
    })
    await page.waitForFunction(
      () =>
        document
          .querySelector('[data-sidebar-announce]')
          ?.textContent?.startsWith('为结算接口补单元测试') &&
        document
          .querySelector('[data-sidebar-announce]')
          ?.textContent?.endsWith('需要你：运行失败'),
      null,
      { timeout: 15_000 },
    )
    report.checks.push(
      'A session newly needing you is announced through the polite live region (“重构登录流程 需要你：等你批准”), also while the sidebar is collapsed to the rail',
    )

    // 窄轨计数：琥珀“需要你”点开全部代理并筛到需要你。
    await page.waitForFunction(
      () => document.querySelector('[data-rail-count="attention"]')?.textContent === '4',
    )
    await page.locator('[data-rail-count="attention"]').click()
    await page.locator('[data-board-root]').waitFor()
    assert.equal(await page.evaluate(() => location.hash), '#/board')
    assert.deepEqual(aborts, [], 'sidebar actions never stop a run')
    await ensureExpanded(page)
    await resetBoardView()
    await context.close()
  }

  // 2b. 同一会话挂着两条普通批准：一步允许第一条后行留在“需要你”（不收起、不隐藏），焦点留在行上，
  // 第二条顶上来，再按 A 处理第二条后行才离开（评审返修：曾把整行永久藏起来，剩下的批准在侧栏看不到）。
  {
    const { context, fixture } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      {
        prepare: (state) => {
          const first = state.live['fx-standard'].approvals[0]
          state.live['fx-standard'].approvals.push({
            ...first,
            id: 'approval-standard-b',
            toolCallId: 'call-standard-b',
            args: { command: 'npm run lint' },
            createdAt: new Date(Date.now() - 10_000).toISOString(),
          })
        },
      },
    )
    const page = await openSession(context, 'two approvals')
    const row = page.locator('[data-sidebar-row="fx-standard"]')
    const item = page.locator('[data-sidebar-item="fx-standard"]')
    await row.hover()
    const approve = item.getByRole('button', { name: /^允许/ })
    await approve.waitFor({ timeout: 20_000 })
    await approve.click()
    await page.waitForFunction(() => document.querySelectorAll('[data-sidebar-item]').length > 0)
    const deadline = Date.now() + 5_000
    while (fixture.posts.length < 1 && Date.now() < deadline) await page.waitForTimeout(100)
    await page.waitForTimeout(800)
    const visible = await page.evaluate(() => {
      const element = document.querySelector(
        '[data-sidebar-group="attention"] [data-sidebar-item="fx-standard"]',
      )
      return element ? { hidden: element.hidden, opacity: getComputedStyle(element).opacity } : null
    })
    assert.deepEqual(
      visible,
      { hidden: false, opacity: '1' },
      'the row stays in 需要你 while another approval waits',
    )
    assert.deepEqual(
      fixture.posts.map((post) => [post.path, post.body]),
      [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
    )
    // 第二条（新的审批 id）就绪后键盘 A 处理它，这时行才离开。
    await row.focus()
    await page.waitForFunction(
      () =>
        document
          .querySelector('[data-sidebar-item="fx-standard"] [data-sidebar-quick] button')
          ?.textContent?.includes('拒绝'),
      null,
      { timeout: 20_000 },
    )
    await page.keyboard.press('KeyA')
    await page.waitForFunction(
      () =>
        !document.querySelector(
          '[data-sidebar-group="attention"] [data-sidebar-item="fx-standard"]:not([hidden])',
        ),
      null,
      { timeout: 10_000 },
    )
    assert.deepEqual(
      fixture.posts.map((post) => post.path),
      [
        '/api/sessions/fx-standard/approvals/approval-standard',
        '/api/sessions/fx-standard/approvals/approval-standard-b',
      ],
    )
    await context.close()
    report.checks.push(
      'A session with two standard approvals: approving the first from the sidebar keeps the row visible in 需要你 (not hidden, opacity 1) with focus on it; A then approves the second and only then the row leaves',
    )
  }

  // 3. 窄轨计数为 0：变灰，不消失。
  {
    const calm = baseSessions().map((session) => ({
      ...session,
      needsAttention: false,
      attentionReason: null,
    }))
    const { context } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      { sessions: calm },
    )
    const page = await openSession(context, 'rail zero')
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="rail"]').waitFor()
    const zero = page.locator('[data-rail-count="attention"]')
    await page.waitForFunction(
      () => document.querySelector('[data-rail-count="attention"]')?.textContent === '0',
    )
    const colors = await page.evaluate(() => {
      const probe = document.createElement('span')
      probe.style.color = 'var(--st-attn-fill)'
      document.body.append(probe)
      const amber = getComputedStyle(probe).color
      probe.style.color = 'var(--text-faint)'
      const faint = getComputedStyle(probe).color
      probe.remove()
      const button = document.querySelector('[data-rail-count="attention"]')
      const glyph = button.querySelector('[data-status-glyph]')
      return {
        amber,
        faint,
        glyph: getComputedStyle(glyph).color,
        background: getComputedStyle(button).backgroundColor,
      }
    })
    assert.equal(await zero.isVisible(), true, 'a zero needs-you count stays on the rail')
    assert.equal(await zero.getAttribute('aria-label'), '0 个代理需要你，打开全部代理')
    assert.notEqual(colors.glyph, colors.amber, 'a zero count is not amber')
    assert.equal(colors.glyph, colors.faint, 'a zero count turns grey')
    assert.equal(colors.background, 'rgba(0, 0, 0, 0)', 'a zero count has no amber chip')
    await ensureExpanded(page)
    await context.close()
    report.checks.push(
      'With nothing needing you the rail still shows the needs-you count as a grey 0 (faint glyph, no amber chip) with its accessible name',
    )
  }

  // 4. 键盘：⌥A → A、聚焦后 D、⌘J（收起时临时展开）、⌥↑ / ⌥↓、F6、返回时焦点回到原会话行。
  {
    // 让普通批准成为等待最久的需要你：失败已处理（留在今天完成），critical 刚到；再加一条普通批准给 D。
    const sessions = baseSessions()
      .map((session) =>
        session.id === 'fx-failed'
          ? { ...session, needsAttention: false, attentionReason: null, unread: false }
          : session,
      )
      .concat({
        ...boardFixture().sessions.find((session) => session.id === 'fx-standard'),
        id: 'fx-standard-2',
        name: '更新接口文档示例',
        firstMessage: '更新接口文档示例',
      })
    const { context, fixture, aborts } = await newContext(
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      {
        sessions,
        prepare: (state) => {
          const recent = new Date(Date.now() - 5_000).toISOString()
          state.live['fx-critical'].approvals[0].createdAt = recent
          state.live['fx-standard-2'] = {
            streaming: false,
            approvals: [
              {
                ...state.live['fx-standard'].approvals[0],
                id: 'approval-standard-2',
                toolCallId: 'call-standard-2',
                args: { command: 'npm run docs' },
                createdAt: new Date(Date.now() - 20_000).toISOString(),
              },
            ],
          }
          state.live['fx-standard'].approvals[0].createdAt = new Date(
            Date.now() - 600_000,
          ).toISOString()
        },
      },
    )
    const page = await openSession(context, 'keyboard')
    const prompt = page.locator('.focus-composer textarea').first()
    // 详情就绪（一步允许可用）后再测 A。
    await page.locator('[data-sidebar-row="fx-standard"]').hover()
    await page
      .locator('[data-sidebar-item="fx-standard"]')
      .getByRole('button', { name: /^允许/ })
      .waitFor({ timeout: 20_000 })
    await page.locator('[data-sidebar-row="fx-standard-2"]').hover()
    await page
      .locator('[data-sidebar-item="fx-standard-2"]')
      .getByRole('button', { name: /^允许/ })
      .waitFor({ timeout: 20_000 })

    await prompt.click()
    await page.keyboard.press('Alt+KeyA')
    await page
      .waitForFunction(
        () => document.activeElement?.getAttribute('data-sidebar-row') === 'fx-standard',
        null,
        { timeout: 10_000 },
      )
      .catch(async (error) => {
        const state = await page.evaluate(() => ({
          active: document.activeElement?.outerHTML.slice(0, 200),
          attention: [
            ...document.querySelectorAll('[data-sidebar-group="attention"] [data-sidebar-row]'),
          ].map((row) => row.getAttribute('aria-label')),
        }))
        throw new Error(`⌥A focus: ${JSON.stringify(state)}`, { cause: error })
      })
    assert.equal(fixture.posts.length, 0, '⌥A only moves focus')
    await page.keyboard.press('KeyA')
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-sidebar-row'))
    await page
      .waitForFunction(
        () =>
          !document.querySelector(
            '[data-sidebar-group="attention"] [data-sidebar-item="fx-standard"]:not([hidden])',
          ),
        null,
        { timeout: 10_000 },
      )
      .catch(async (error) => {
        const state = await page.evaluate(() => ({
          active: document.activeElement?.outerHTML.slice(0, 300),
          item: document
            .querySelector('[data-sidebar-item="fx-standard"]')
            ?.outerHTML.slice(0, 600),
        }))
        throw new Error(`A: ${JSON.stringify({ state, posts: fixture.posts })}`, { cause: error })
      })
    assert.deepEqual(
      fixture.posts.map((post) => [post.path, post.body]),
      [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
    )
    assert.notEqual(await activeRow(page), 'fx-standard', 'focus moves on to a following row')
    await page.locator('[data-sidebar-row="fx-standard-2"]').focus()
    await page.keyboard.press('KeyD')
    await page.waitForFunction(
      () =>
        !document.querySelector(
          '[data-sidebar-group="attention"] [data-sidebar-item="fx-standard-2"]:not([hidden])',
        ),
    )
    assert.deepEqual(fixture.posts.at(-1).body, { approved: false })
    assert.equal(fixture.posts.length, 2)
    // critical 行聚焦后 A 什么都不发。
    await page.locator('[data-sidebar-row="fx-critical"]').focus()
    await page.keyboard.press('KeyA')
    await page.waitForTimeout(300)
    assert.equal(fixture.posts.length, 2, 'A on a critical row sends nothing')
    report.checks.push(
      '⌥A from the composer focuses the longest-waiting standard approval without approving; A then approves it with one POST and focus moves to the next row; D on a focused standard approval denies it; A on a critical row sends nothing',
    )

    // ⌘J：收起时临时展开并聚焦当前会话行，Esc 收回窄轨，收起偏好不变。
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="rail"]').waitFor()
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.locator('[data-shell-sidebar-peek]').waitFor()
    await page.waitForFunction(
      (id) => document.activeElement?.getAttribute('data-sidebar-row') === id,
      BOARD_SESSION_ID,
    )
    await page.keyboard.press('Escape')
    await page.locator('[data-shell-sidebar-peek]').waitFor({ state: 'detached' })
    assert.equal(await page.locator('[data-shell-sidebar="rail"]').count(), 1)
    // ⌥A 收起时同样临时展开。
    await prompt.click()
    await page.keyboard.press('Alt+KeyA')
    await page.locator('[data-shell-sidebar-peek]').waitFor()
    await page.waitForFunction(
      () =>
        document.activeElement?.closest('[data-shell-sidebar-peek]') &&
        document.activeElement?.getAttribute('data-sidebar-group-row') === 'attention',
    )
    await prompt.click()
    await page.locator('[data-shell-sidebar-peek]').waitFor({ state: 'detached' })
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="expanded"]').waitFor()
    await prompt.click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForFunction(
      (id) => document.activeElement?.getAttribute('data-sidebar-row') === id,
      BOARD_SESSION_ID,
    )
    report.checks.push(
      '⌘B collapses and expands; with the sidebar collapsed ⌘J and ⌥A temporarily expand it as an overlay focused on the current / longest-waiting row and Esc or focusing elsewhere folds it back without changing the collapsed preference; expanded ⌘J focuses the current session row',
    )

    // ⌥↓ / ⌥↑：按侧栏可见顺序、replace（历史长度不变）。
    const order = await page.evaluate(() =>
      [...document.querySelectorAll('[data-sidebar-groups] [data-sidebar-row]')].map((row) =>
        row.getAttribute('data-sidebar-row'),
      ),
    )
    const index = order.indexOf(BOARD_SESSION_ID)
    assert.ok(index >= 0 && index < order.length - 1, `current session sits inside ${order}`)
    const historyBefore = await page.evaluate(() => history.length)
    await prompt.click()
    await page.keyboard.press('Alt+ArrowDown')
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, order[index + 1])
    await page.locator(`[data-sidebar-row="${order[index + 1]}"][aria-current="page"]`).waitFor()
    await page.keyboard.press('Alt+ArrowUp')
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    await page.locator('.focus-composer textarea').first().waitFor()
    // 连按：第二下以还没提交的目标为起点。
    await page.keyboard.press('Alt+ArrowDown')
    await page.keyboard.press('Alt+ArrowDown')
    await page.waitForFunction(
      (id) => location.hash === `#/board/${id}`,
      order[index + 2] ?? order[index + 1],
    )
    await page.keyboard.press('Alt+ArrowUp')
    await page.keyboard.press('Alt+ArrowUp')
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    assert.equal(await page.evaluate(() => history.length), historyBefore, '⌥↑⌥↓ replace the route')
    report.checks.push(
      '⌥↓ / ⌥↑ walk the sessions in the sidebar’s visible order with replace navigation (history length unchanged); back-to-back presses start from the switch that has not committed yet',
    )

    // F6：输入框 → 侧栏（当前会话行）→ 顶栏 → 对话 → 输入框。
    await page.locator('[data-session-header]').first().waitFor()
    await page.getByText('已完成重构，全部测试通过。').first().waitFor({ timeout: 30_000 })
    await prompt.click()
    const regions = []
    for (const expected of ['sidebar', 'header', 'content', 'composer']) {
      await page.keyboard.press('F6')
      await page.waitForFunction(
        (region) =>
          document.activeElement
            ?.closest('[data-focus-region]')
            ?.getAttribute('data-focus-region') === region,
        expected,
      )
      regions.push(await activeRegion(page))
    }
    assert.deepEqual(regions, ['sidebar', 'header', 'content', 'composer'])
    assert.equal(
      await page.evaluate(() => document.activeElement?.tagName),
      'TEXTAREA',
      'F6 lands in the composer text box',
    )
    await page.keyboard.press('F6')
    await page.waitForFunction(
      (id) => document.activeElement?.getAttribute('data-sidebar-row') === id,
      BOARD_SESSION_ID,
    )
    report.checks.push(
      'F6 cycles focus composer → sidebar (current session row) → session header → transcript → composer text box → sidebar',
    )

    // 返回时焦点回到原会话行：⌘1 去全部代理、⌘, 去设置，浏览器后退回来。
    for (const [key, prefix] of [
      ['Digit1', '#/board'],
      ['Comma', '#/config/'],
    ]) {
      await prompt.click()
      await page.keyboard.press(`${mod}+${key}`)
      await page.waitForFunction(
        (start) =>
          start.endsWith('/') ? location.hash.startsWith(start) : location.hash === start,
        prefix,
      )
      await page.locator('[data-focus-region="header"]').first().waitFor()
      await settle(page)
      await page.goBack()
      await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
      await page.waitForFunction(
        (id) => document.activeElement?.getAttribute('data-sidebar-row') === id,
        BOARD_SESSION_ID,
      )
    }
    report.checks.push(
      'Leaving the session for all agents (⌘1) or settings (⌘,) and coming back puts focus on the original session row in the sidebar',
    )
    assert.deepEqual(aborts, [], 'no sidebar shortcut stops a run')
    await ensureExpanded(page)
    await context.close()
  }

  // 5. 侧栏随页面变化（A3，规格 2.3 / 2.6）：全部代理页的筛选形态与设置整页。
  {
    const { context, aborts } = await newContext({
      viewport: { width: 1440, height: 900 },
      colorScheme: 'light',
    })
    const page = await openSession(context, 'page-shaped sidebar')
    const prompt = page.locator('.focus-composer textarea').first()
    const sidebar = page.locator('[data-shell-sidebar]')
    const active = () =>
      page.evaluate(() => {
        const element = document.activeElement
        return {
          filter: element?.getAttribute('data-sidebar-filter') ?? null,
          card: element?.getAttribute('data-board-card') ?? null,
          lane: element?.getAttribute('data-lane') ?? null,
          section: element?.getAttribute('data-settings-section') ?? null,
          fleet: element?.hasAttribute('data-sidebar-fleet-line') ?? false,
          row: element?.getAttribute('data-sidebar-row') ?? null,
        }
      })

    // 5a. 全部代理页：侧栏不列会话行，只有状态 / 项目筛选，计数与侧栏分组一致（需要你 3、运行中 3）。
    await prompt.click()
    await page.keyboard.press(`${mod}+Digit1`)
    await page.waitForFunction(() => location.hash === '#/board')
    await sidebar.locator('[data-sidebar-filters]').waitFor()
    await page.waitForFunction(
      () =>
        document.querySelector('[data-sidebar-filter="attention"] [data-sidebar-filter-count]')
          ?.textContent === '3' &&
        document.querySelector('[data-sidebar-filter="running"] [data-sidebar-filter-count]')
          ?.textContent === '3',
    )
    assert.equal(await sidebar.locator('[data-sidebar-row]').count(), 0)
    assert.deepEqual(
      await sidebar
        .locator('[data-sidebar-filter]:not([data-sidebar-filter^="project:"])')
        .evaluateAll((nodes) => nodes.map((node) => node.dataset.sidebarFilter)),
      ['all', 'attention', 'running', 'done'],
    )
    assert.equal(
      await sidebar.locator('[data-sidebar-entry="board"]').getAttribute('aria-current'),
      'page',
    )
    await geometry(page, 'all agents filters 1440')
    // ⌘J 聚焦按下的筛选（全部）；⌥A 把焦点交给主区等待最久的那一行，不批准、不导航。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForFunction(
      () => document.activeElement?.getAttribute('data-sidebar-filter') === 'all',
    )
    await page.keyboard.press('ArrowDown')
    await page.keyboard.press('ArrowDown')
    assert.equal((await active()).filter, 'running')
    await page.keyboard.press('Enter')
    await page.waitForFunction(
      () =>
        document.querySelector('[data-sidebar-filter="running"]')?.getAttribute('aria-pressed') ===
        'true',
    )
    await page.locator('[data-board-root] [data-lane="attention"]').first().waitFor({
      state: 'detached',
    })
    // 被“运行中”筛选藏起来的需要你：⌥A 先清掉筛选再聚焦那一行。
    await page.keyboard.press('Alt+KeyA')
    await page.waitForFunction(
      () =>
        document.activeElement?.getAttribute('data-lane') === 'attention' &&
        document.activeElement?.closest('[data-overview]') !== null,
    )
    assert.equal(
      await sidebar.locator('[data-sidebar-filter="all"]').getAttribute('aria-pressed'),
      'true',
    )
    assert.equal(await page.evaluate(() => location.hash), '#/board')
    // 收起成窄轨时 ⌘J 临时展开，焦点落到筛选上；Esc 收回。
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="rail"]').waitFor()
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.locator('[data-shell-sidebar-peek] [data-sidebar-filters]').waitFor()
    await page.waitForFunction(
      () =>
        document.activeElement?.closest('[data-shell-sidebar-peek]') &&
        document.activeElement?.hasAttribute('data-sidebar-filter'),
    )
    await page.keyboard.press('Escape')
    await page.locator('[data-shell-sidebar-peek]').waitFor({ state: 'detached' })
    await page.screenshot({ path: join(output, 'all-agents-filters-rail.png') })
    report.checks.push(
      'All agents page: the sidebar lists no sessions, only status filters (全部 · 需要你 3 · 运行中 3 · 今天完成; no 排队 without sessionQueue) and project filters; ⌘J focuses the pressed filter, ↓ / ↵ pick 运行中, ⌥A clears the filter that hid the longest-waiting row and focuses it in the main list without approving; collapsed, ⌘J peeks the filters and Esc folds them back',
    )

    // 5b. 设置整页（收起状态下也整块 268）：“← 返回 Esc”、设置目录 6 项、搜索、功能链接、底部状态行；
    // 主区没有第三列导航。切换分区后 Esc 一次退回全部代理（进入设置前的那一页）。
    await page.locator('[data-board-root] h1').click()
    await page.keyboard.press(`${mod}+Comma`)
    await page.waitForFunction(() => location.hash === '#/config/models')
    const settings = page.locator('[data-shell-sidebar="settings"]')
    await settings.locator('[data-settings-sidebar]').waitFor()
    await settings.locator('[data-sidebar-fleet-line]').waitFor()
    const shell = await page.evaluate(() => ({
      width: Math.round(
        document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect().width ?? 0,
      ),
      sections: [...document.querySelectorAll('[data-settings-section]')].map((button) => [
        button.getAttribute('data-settings-section'),
        button.getAttribute('aria-current'),
      ]),
      back: document.querySelector('[data-settings-back]')?.textContent?.trim(),
      entries: document.querySelectorAll('[data-shell-sidebar] [data-sidebar-entry]').length,
      // 设置导航只在侧栏里，主区不再叠第三列。
      thirdColumn: [...document.querySelectorAll('[aria-label="设置导航"]')].filter(
        (element) => !element.closest('[data-shell-sidebar]'),
      ).length,
      search: Boolean(document.querySelector('[data-settings-sidebar] input')),
      placeholder: document
        .querySelector('[data-settings-sidebar] input')
        ?.getAttribute('placeholder'),
      labels: [...document.querySelectorAll('[data-settings-section]')].map((button) =>
        button.textContent.trim(),
      ),
      hint: document.querySelector('[data-settings-features-link]')?.textContent?.trim(),
      fleet: document.querySelector('[data-sidebar-fleet-line]')?.textContent?.trim(),
      fleetLabel: document.querySelector('[data-sidebar-fleet-line]')?.getAttribute('aria-label'),
    }))
    // 目录是短名（规格 2.6），“关于”后面跟版本号；侧栏搜索框用短占位，不被截成半个英文词。
    assert.deepEqual(shell.labels.slice(0, 5), ['模型', '界面', '通知', '快捷键', '更新'])
    assert.match(shell.labels[5], /^关于/)
    assert.equal(shell.placeholder, '搜索设置')
    assert.deepEqual(
      {
        ...shell,
        fleet: undefined,
        fleetLabel: undefined,
        labels: undefined,
        placeholder: undefined,
      },
      {
        width: 268,
        sections: [
          ['models', 'page'],
          ['interface', null],
          ['notifications', null],
          ['shortcuts', null],
          ['updates', null],
          ['about', null],
        ],
        back: '返回',
        entries: 0,
        thirdColumn: 0,
        search: true,
        hint: '插件、MCP、技能等在“功能”里',
        fleet: undefined,
        fleetLabel: undefined,
        labels: undefined,
        placeholder: undefined,
      },
    )
    assert.match(shell.fleet, /3 个需要你\s*·\s*3 个运行中/)
    assert.equal(shell.fleetLabel, '3 个代理需要你，3 个运行中，打开全部代理')
    await geometry(page, 'settings shell 1440 (rail preference)')
    await page.screenshot({ path: join(output, 'settings-shell-light.png') })
    await settings.locator('[data-settings-section="interface"]').click()
    await page.waitForFunction(() => location.hash === '#/config/interface')
    assert.equal(
      await settings.locator('[data-settings-section="interface"]').getAttribute('aria-current'),
      'page',
    )
    // 设置搜索框里的 Esc 只清空 / 收起搜索，不离开设置。
    const search = settings.locator('input').first()
    await search.fill('主题')
    await page.keyboard.press('Escape')
    await page.keyboard.press('Escape')
    await page.waitForTimeout(200)
    assert.equal(await page.evaluate(() => location.hash), '#/config/interface')
    // 焦点不在输入框：Esc 回到进入设置前的全部代理（一次退回，不是上一个分区）。
    await settings.locator('[data-settings-section="interface"]').focus()
    await page.keyboard.press('Escape')
    await page.waitForFunction(() => location.hash === '#/board')
    await page.locator('[data-shell-sidebar="rail"]').waitFor()
    report.checks.push(
      'Settings take over the sidebar as a full 268px page even with the rail preference: ← 返回 + Esc, 设置 with exactly 模型 · 界面 · 通知 · 快捷键 · 更新 · 关于 (aria-current on the open section), the settings search, “插件、MCP、技能等在“功能”里 →” and “● 3 个需要你 · ◠ 3 个运行中 ›”; no entries, no third navigation column, top-right 144px free; Esc inside the search box stays, Esc elsewhere goes back to 全部代理 in one step after switching sections, and the rail comes back',
    )
    await page.keyboard.press(`${mod}+KeyB`)
    await page.locator('[data-shell-sidebar="expanded"]').waitFor()

    // 5c. 从会话进设置：“← 返回”回到原会话、焦点回到原会话行；底部状态行打开全部代理（清掉状态筛选）。
    await page.goto(`${base}/#/board/${BOARD_SESSION_ID}`)
    await prompt.waitFor()
    await sidebar.locator(`[data-sidebar-row="${BOARD_SESSION_ID}"]`).waitFor()
    await prompt.click()
    await page.keyboard.press(`${mod}+Comma`)
    await settings.locator('[data-settings-sidebar]').waitFor()
    await settings.locator('[data-settings-section="notifications"]').click()
    await page.waitForFunction(() => location.hash === '#/config/notifications')
    await settings.locator('[data-settings-back]').click()
    await page.waitForFunction((id) => location.hash === `#/board/${id}`, BOARD_SESSION_ID)
    await page.waitForFunction(
      (id) => document.activeElement?.getAttribute('data-sidebar-row') === id,
      BOARD_SESSION_ID,
    )
    // ⌥A / ⌘J 在设置里：落到底部状态行 / 当前分区，不临时展开别的东西。
    await prompt.click()
    await page.keyboard.press(`${mod}+Comma`)
    await settings.locator('[data-sidebar-fleet-line]').waitFor()
    await page.locator('main').click({ position: { x: 600, y: 600 } })
    await page.keyboard.press('Alt+KeyA')
    await page.waitForFunction(() =>
      document.activeElement?.hasAttribute('data-sidebar-fleet-line'),
    )
    await page.keyboard.press(`${mod}+KeyJ`)
    await page.waitForFunction(
      () => document.activeElement?.getAttribute('data-settings-section') === 'models',
    )
    await settings.locator('[data-sidebar-fleet-line]').click()
    await page.waitForFunction(() => location.hash === '#/board')
    await sidebar.locator('[data-sidebar-filters]').waitFor()
    assert.equal(
      await sidebar.locator('[data-sidebar-filter="all"]').getAttribute('aria-pressed'),
      'true',
    )
    // 从全部代理去设置（Esc 返回）或功能（⌘2 → ⌘1）再回来：焦点回到原来的那一行（规格 2.3）。
    const boardRowId = await page
      .locator('[data-overview] [data-board-card]')
      .nth(1)
      .getAttribute('data-board-card')
    const focusedBoardRow = (id) =>
      page.waitForFunction(
        (target) => document.activeElement?.getAttribute('data-board-card') === target,
        id,
        { timeout: 10_000 },
      )
    await page.locator(`[data-overview] [data-board-card="${boardRowId}"]`).focus()
    await page.keyboard.press(`${mod}+Comma`)
    await settings.locator('[data-settings-sidebar]').waitFor()
    await page.keyboard.press('Escape')
    await page.waitForFunction(() => location.hash === '#/board')
    await focusedBoardRow(boardRowId)
    await page.keyboard.press(`${mod}+Digit2`)
    await page.waitForFunction(() => location.hash === '#/more')
    await page.keyboard.press(`${mod}+Digit1`)
    await page.waitForFunction(() => location.hash === '#/board')
    await focusedBoardRow(boardRowId)
    report.checks.push(
      'From a session, ⌘, then 通知 then “← 返回” goes back to the same session and focus returns to its sidebar row; in settings ⌥A focuses the bottom status line and ⌘J the open section; the status line opens 全部代理 with the status filter cleared; from 全部代理, ⌘, then Esc and ⌘2 then ⌘1 both return focus to the row that had it',
    )

    // 5d. 双归属退役：远程访问（/config 下）与插件是功能页——点亮“功能”、页头“‹ 功能”、侧栏是常规会话分组。
    for (const path of ['/config/remote-access', '/plugins']) {
      await page.goto(`${base}/#${path}`)
      await page.locator('[data-back-to-more]').waitFor({ timeout: 30_000 })
      assert.equal(await page.locator('[data-shell-sidebar="settings"]').count(), 0, path)
      assert.equal(
        await sidebar.locator('[data-sidebar-entry="features"]').getAttribute('aria-current'),
        'page',
        path,
      )
      assert.equal(await page.locator('[data-back-to-more]').innerText(), '功能')
      assert.equal(await page.locator('[data-back-to-more]').getAttribute('aria-label'), '返回功能')
    }
    await page.locator('[data-back-to-more]').click()
    await page.waitForFunction(() => location.hash === '#/more')
    // 设置目录里的“在功能里 →”同样去功能。
    await page.keyboard.press(`${mod}+Comma`)
    await settings.locator('[data-settings-features-link]').click()
    await page.waitForFunction(() => location.hash === '#/more')
    await page.locator('[data-shell-sidebar="expanded"]').waitFor()
    report.checks.push(
      'Remote access (/config/remote-access) and plugins open as feature pages: 功能 is lit, the header shows “‹ 功能” (返回功能) back to the index, no settings sidebar; the settings link “在功能里 →” opens the index too',
    )

    // 深色下的设置整页截图（对照用）。
    await page.emulateMedia({ colorScheme: 'dark' })
    await page.keyboard.press(`${mod}+Comma`)
    await settings.locator('[data-sidebar-fleet-line]').waitFor()
    await settle(page)
    await geometry(page, 'settings shell 1440 dark')
    await page.screenshot({ path: join(output, 'settings-shell-dark.png') })
    assert.deepEqual(aborts, [], 'no page-shaped sidebar action stops a run')
    await resetBoardView()
    await ensureExpanded(page)
    await context.close()
  }

  // 6. 功能索引（A3，规格 2.7）：居中 760 栏、两列卡片行、分组顺序与组件工作室的位置、角标（只有渠道断开是琥珀）、
  //    “/”与方向键、搜索与空结果，以及每张卡片打开的功能页都点亮“功能”、页头“‹ 功能”并能退回索引。
  {
    const { context } = await newContext({
      viewport: { width: 1440, height: 900 },
      colorScheme: 'light',
    })
    // 角标只在浏览器边界模拟四个读接口的计数；逐页打开之前撤掉，功能页读真实 Runtime。
    const badgeRoutes = [
      [
        '**/api/workflows',
        (route) =>
          route.request().method() === 'GET'
            ? route.fulfill({
                json: { workflows: [], runs: [], limits: { maxConcurrent: 4, running: 1 } },
              })
            : route.fallback(),
      ],
      [
        '**/api/schedules',
        (route) =>
          route.request().method() === 'GET'
            ? route.fulfill({
                json: {
                  tasks: [
                    { id: 'a', enabled: true },
                    { id: 'b', enabled: true },
                    { id: 'c', enabled: false },
                  ],
                  runs: [],
                },
              })
            : route.fallback(),
      ],
      [
        '**/api/channels',
        (route) =>
          route.request().method() === 'GET'
            ? route.fulfill({
                json: {
                  providers: [],
                  connections: {
                    feishu: { enabled: true, status: 'failed' },
                    weixin: { enabled: true, status: 'connected' },
                  },
                  scopes: [],
                  models: [],
                },
              })
            : route.fallback(),
      ],
      [
        '**/api/mcp?refresh=0',
        (route) =>
          route.fulfill({
            json: {
              services: [],
              tools: [],
              calls: [],
              metrics: {
                totalServices: 3,
                onlineServices: 3,
                availableTools: 0,
                restrictedTools: 0,
                errorRate: 0,
              },
            },
          }),
      ],
    ]
    for (const [url, handler] of badgeRoutes) await context.route(url, handler)
    const page = await openSession(context, 'features index')
    const sidebar = page.locator('[data-shell-sidebar]')
    const focusedTile = () =>
      page.evaluate(() => document.activeElement?.getAttribute('data-more-tile') ?? null)

    // 6a. ⌘2 打开功能：侧栏点亮“功能”，标题与文档标题都是“功能”，分组顺序与组件工作室在“扩展”末尾。
    await page.locator('.focus-composer textarea').first().click()
    await page.keyboard.press(`${mod}+Digit2`)
    await page.waitForFunction(() => location.hash === '#/more')
    await page.locator('[data-more-badge]').nth(3).waitFor({ timeout: 20_000 })
    assert.equal(
      await sidebar.locator('[data-sidebar-entry="features"]').getAttribute('aria-current'),
      'page',
    )
    assert.equal(await page.locator('main h1').first().innerText(), '功能')
    assert.equal(await page.title(), '功能 · Pisper')
    assert.equal(
      await page.locator('#more-page-search').getAttribute('placeholder'),
      '搜索功能、设置与命令',
    )
    const layout = await page.evaluate(() => {
      const main = document.querySelector('main').getBoundingClientRect()
      const groups = [...document.querySelectorAll('[data-more-group]')].map((group) => ({
        id: group.getAttribute('data-more-group'),
        heading: group.querySelector('h2').textContent,
        tiles: [...group.querySelectorAll('[data-more-tile]')].map((tile) => {
          const box = tile.getBoundingClientRect()
          return {
            key: tile.getAttribute('data-more-tile'),
            left: Math.round(box.left),
            top: Math.round(box.top),
            width: Math.round(box.width),
            height: Math.round(box.height),
            border: getComputedStyle(tile).borderTopStyle,
          }
        }),
      }))
      const list = document.querySelector('[data-more-group] ul').getBoundingClientRect()
      return {
        groups,
        listWidth: Math.round(list.width),
        offCenter: Math.abs(list.left + list.width / 2 - (main.left + main.width / 2)),
      }
    })
    assert.deepEqual(
      layout.groups.map((group) => [group.id, group.heading]),
      [
        ['sessions', '会话与资料'],
        ['automation', '自动化'],
        ['extensions', '扩展'],
        ['devices', '设备与连接'],
        ['components', '我的组件'],
      ],
    )
    const keysOf = (id) => layout.groups.find((group) => group.id === id).tiles.map((t) => t.key)
    assert.deepEqual(keysOf('sessions'), ['page:chatHistory', 'page:assets', 'page:memory'])
    assert.deepEqual(keysOf('automation'), ['page:workflows', 'page:schedules', 'page:channels'])
    assert.deepEqual(keysOf('extensions'), [
      'page:plugins',
      'page:skills',
      'page:mcp',
      'page:components',
    ])
    assert.equal(keysOf('components').at(-1), 'add:component')
    // 居中 760 栏、两列：同一行两张等宽卡片（(760 − 8) / 2），第三张换到下一行左列；卡片行至少 64 高。
    assert.equal(layout.listWidth, 760)
    assert.ok(layout.offCenter <= 1, `features column is centered (${layout.offCenter})`)
    const [first, second, third] = layout.groups[0].tiles
    assert.equal(first.top, second.top)
    assert.equal(third.left, first.left)
    assert.ok(third.top > first.top)
    assert.deepEqual([first.width, second.width], [376, 376])
    const allTiles = layout.groups.flatMap((group) => group.tiles)
    assert.ok(
      allTiles.every((tile) => tile.height >= 64),
      JSON.stringify(allTiles),
    )
    // 只有“+ 放置组件”是虚线空位；组件工作室是普通入口。
    assert.deepEqual(
      allTiles.filter((tile) => tile.border === 'dashed').map((tile) => tile.key),
      ['add:component'],
    )
    // 角标：渠道“1 断开”是唯一的琥珀（--warning-strong 文字 + 琥珀圆点），其余是中性计数。
    const badges = await page.locator('[data-more-badge]').evaluateAll((nodes) => {
      const probe = document.createElement('span')
      probe.style.color = 'var(--warning-strong)'
      document.body.append(probe)
      const amber = getComputedStyle(probe).color
      probe.remove()
      return nodes.map((node) => ({
        key: node.closest('[data-more-tile]').dataset.moreTile,
        tone: node.dataset.moreBadge,
        text: node.textContent.trim(),
        amber: getComputedStyle(node).color === amber,
        dot: node.querySelector('.rounded-full') !== null,
      }))
    })
    assert.deepEqual(
      badges.toSorted((a, b) => a.key.localeCompare(b.key)),
      [
        { key: 'page:channels', tone: 'attn', text: '1 断开', amber: true, dot: true },
        { key: 'page:mcp', tone: 'neutral', text: '3 在线', amber: false, dot: false },
        { key: 'page:schedules', tone: 'neutral', text: '2 启用', amber: false, dot: false },
        { key: 'page:workflows', tone: 'run', text: '1 运行中', amber: false, dot: false },
      ],
    )
    await settle(page)
    await geometry(page, 'features 1440 light')
    await page.screenshot({ path: join(output, 'features-1440-light.png') })
    report.checks.push(
      'Features (⌘2): 功能 lit, h1 and document title 功能, groups 会话与资料 · 自动化 · 扩展 (with 组件工作室 last) · 设备与连接 · 我的组件 (ending in the dashed + 放置组件); a centered 760 column of two equal 376px card columns ≥64px tall; only the channel badge “1 断开” is amber, the other counts are neutral',
    )

    // 6b. 键盘：“/”聚焦搜索，↓ 进第一张卡片，→ / ↓ / ↑ 在两列之间按位置移动，第一行 ↑ 回搜索框；焦点环 2px。
    await page.locator('main h1').first().click()
    await page.keyboard.press('Slash')
    await page.waitForFunction(() => document.activeElement?.id === 'more-page-search')
    await page.keyboard.press('ArrowDown')
    assert.equal(await focusedTile(), 'page:chatHistory')
    const ring = await page.evaluate(() => {
      const style = getComputedStyle(document.activeElement)
      return `${style.outlineStyle} ${style.outlineWidth}`
    })
    assert.equal(ring, 'solid 2px')
    await page.keyboard.press('ArrowRight')
    assert.equal(await focusedTile(), 'page:assets')
    await page.keyboard.press('ArrowDown')
    assert.equal(await focusedTile(), 'page:memory')
    await page.keyboard.press('ArrowDown')
    assert.equal(await focusedTile(), 'page:workflows')
    await page.keyboard.press('ArrowUp')
    assert.equal(await focusedTile(), 'page:memory')
    await page.keyboard.press('Home')
    assert.equal(await focusedTile(), 'page:chatHistory')
    await page.keyboard.press('ArrowUp')
    await page.waitForFunction(() => document.activeElement?.id === 'more-page-search')
    await page.keyboard.press('End')
    // 在搜索框里 End 只是移动光标；Tab 依次走到每张卡片（禁用的也能聚焦，读得到原因）。
    await page.keyboard.press('Tab')
    assert.equal(await focusedTile(), 'page:chatHistory')
    await page.keyboard.press('Tab')
    assert.equal(await focusedTile(), 'page:assets')

    // 6c. 搜索：关键词筛卡片并列出命中的设置项；↓ ↵ 打开结果；空结果写明并可清除；Esc 清空。
    await page.keyboard.press('Slash')
    await page.keyboard.type('mcp')
    await page.waitForFunction(
      () =>
        [...document.querySelectorAll('[data-more-tile]')]
          .map((tile) => tile.getAttribute('data-more-tile'))
          .join() === 'page:mcp',
    )
    await page.locator('[data-more-palette-search]').waitFor()
    await page.keyboard.press('ArrowDown')
    assert.equal(await focusedTile(), 'page:mcp')
    await page.keyboard.press('Enter')
    await page.waitForFunction(() => location.hash === '#/mcp')
    await page.locator('[data-back-to-more]').click()
    await page.waitForFunction(() => location.hash === '#/more')
    await page.locator('#more-page-search').fill('没有这个功能-zz')
    const empty = page.locator('[data-features-empty]')
    await empty.waitFor()
    assert.equal(await empty.getAttribute('role'), 'status')
    await empty.getByRole('button', { name: '清除搜索', exact: true }).click()
    await page.locator('[data-more-tile="page:chatHistory"]').waitFor()
    await page.locator('#more-page-search').fill('技能')
    await page.locator('#more-page-search').press('Escape')
    assert.equal(await page.locator('#more-page-search').inputValue(), '')
    assert.equal(page.url().endsWith('#/more'), true, 'Esc in the search box clears, never leaves')
    report.checks.push(
      'Features keyboard: “/” focuses the search, ↓ enters the first card (2px focus ring), → / ↓ / ↑ move by position across the two columns and groups, Home goes to the first card, ↑ from the first row returns to the search, Tab walks the cards; typing “mcp” leaves only MCP plus the palette hand-off, ↓ ↵ opens it; no match shows a status with 清除搜索; Esc clears the query without leaving',
    )

    // 6d. 每张卡片打开的功能页（含组件工作室与 /config 下的远程访问 / 桌面宠物）都点亮“功能”、
    //     页头“‹ 功能”（返回功能），点它回到索引。
    for (const [url, handler] of badgeRoutes) await context.unroute(url, handler)
    const destinations = allTiles
      .map((tile) => tile.key)
      .filter((key) => key !== 'add:component' && !key.startsWith('component:'))
    const opened = []
    for (const key of destinations) {
      const tile = page.locator(`[data-more-tile="${key}"]`)
      await tile.waitFor()
      if ((await tile.getAttribute('aria-disabled')) === 'true') continue
      await tile.click()
      await page.waitForFunction(() => location.hash !== '#/more')
      const back = page.locator('[data-back-to-more]')
      await back.waitFor({ timeout: 30_000 })
      assert.equal(await back.innerText(), '功能', key)
      assert.equal(await back.getAttribute('aria-label'), '返回功能', key)
      assert.equal(
        await sidebar.locator('[data-sidebar-entry="features"]').getAttribute('aria-current'),
        'page',
        key,
      )
      opened.push(`${key} → ${new URL(page.url()).hash}`)
      await back.click()
      await page.waitForFunction(() => location.hash === '#/more')
      // 回到索引后焦点还给来源卡片（规格 2.3）。
      await page.waitForFunction(
        (target) => document.activeElement?.getAttribute('data-more-tile') === target,
        key,
        { timeout: 10_000 },
      )
    }
    assert.ok(opened.some((entry) => entry.startsWith('page:components → #/components')))
    assert.ok(opened.some((entry) => entry.startsWith('config:remote-access → #/config/')))
    report.features = opened
    report.checks.push(
      `Every features card opens its page with 功能 lit and “‹ 功能” (返回功能) back to the index, focus returning to the card: ${opened.length} pages incl. 组件工作室 and /config/remote-access`,
    )

    // 6e. 1080 宽：栏宽随主区收窄（仍两列、居中、无横向滚动）；深色截图。高度取第 1 节同一档 680
    //     （侧栏列表在设置行下面滚动，720 高时“今天完成”最后一行正好半截压在设置行下，几何自查会把它算作被挡）。
    await page.setViewportSize({ width: 1080, height: 680 })
    await settle(page)
    const narrow = await page.evaluate(() => {
      const tiles = [...document.querySelectorAll('[data-more-group="sessions"] [data-more-tile]')]
      const [a, b] = tiles.map((tile) => tile.getBoundingClientRect())
      return { sameRow: Math.round(a.top) === Math.round(b.top), width: Math.round(a.width) }
    })
    assert.equal(narrow.sameRow, true)
    assert.ok(narrow.width >= 300, `1080 card width ${narrow.width}`)
    await geometry(page, 'features 1080 light')
    await page.screenshot({ path: join(output, 'features-1080-light.png') })
    await page.setViewportSize({ width: 1440, height: 900 })
    await page.emulateMedia({ colorScheme: 'dark' })
    await settle(page)
    await geometry(page, 'features 1440 dark')
    await page.screenshot({ path: join(output, 'features-1440-dark.png') })
    report.checks.push(
      'Features at 1080: still two columns side by side, no horizontal scroll, top-right 144px free; dark screenshot',
    )
    await context.close()
  }

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
  console.log(`Sidebar smoke report and screenshots: ${output}`)
}
