// 侧栏冒烟（规格 v2 第 2.1、2.2、2.3、2.8、4.6、9.2 节，A1 验收）：真实生产构建 + 隔离的临时 Runtime；
// 会话列表、/live 与审批只在浏览器边界用固定数据提供（scripts/fixtures/board-fixture.mjs），
// 会话页的那条会话写进本次的临时数据目录，不连接模型、不写真实数据。
// 覆盖：1440×900 / 1080×680 × 浅色 / 深色 × 展开 / 收起，模拟 Windows 自绘标题栏时右上 144px 没有可交互元素、
// 没有横向滚动；窄轨计数（为 0 时变灰不消失）；需要你行的读屏名称与新增需要你的 polite 播报（收起时也播）；
// 快捷处理（悬停 / 聚焦后普通批准一步允许或拒绝，只发一次请求、行收起）；⌥A 聚焦等待最久的行 → A 批准；
// ⌥↑ / ⌥↓ 按侧栏顺序 replace 切换；⌘B；⌘J（收起时临时展开）；F6 在侧栏、顶栏、对话、输入框之间切换；
// 从全部代理 / 设置返回时焦点回到原会话行；1x 截图里运行中的弧与完成逐像素可区分（排队见 smoke-replica-shell）。
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
        '[data-shell-sidebar] [data-sidebar-entry], [data-shell-sidebar] [data-sidebar-row], [data-shell-sidebar] [data-rail-count]',
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
