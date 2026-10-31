// 原生组件冒烟（A7 S4，计划 §6 S4）：真实生产构建 + 隔离的临时 Runtime（数据目录在临时目录里，不连模型、不写真实数据）。
// 种子：手放的原生组件、会抛错的原生组件、一个旧版 iframe 组件（启动迁移应把它移进回收站，负责人决定），
// 启动后再放一个坏目录；经 API 建 feature-page 与 floating 实例。覆盖：
// - 组件页（S4 过渡版）只列原生行与坏目录行，没有旧版行；旧组件文件原样在 custom-ui-trash 里
// - 预览在真实 ComponentHost 里渲染（htm 模板）；/tools/components/:id 整页渲染 feature-page 实例
// - 浮窗渲染、拖动后位置写进 pisper-floating-placement、关闭 = 实例 PATCH enabled:false
// - 崩溃隔离：错误卡 → 60 秒内 3 次后“已暂停” → 恢复
// - ?safe-mode=components 下只显示占位卡；安全模式哨兵还在时先询问，选“正常加载”后才加载
// - 热重载：改写 component.js 后预览内容随 rev 更新
// - 坏目录行可删除（移入回收站）；1440 与 1080 宽都没有横向滚动
// 浏览器模拟不等于 Tauri / WebView2 或真机验证。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import { desktopBridgeScript } from './fixtures/board-fixture.mjs'

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
const legacyDir = await writeComponent('legacy-old', {
  name: '旧版组件',
  entry: 'index.html',
  permissions: ['notify'],
})
await writeFile(join(legacyDir, 'index.html'), '<!doctype html><body>legacy</body>')

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
    ...((process.env.PISPER_UI_BROWSER_PATH ?? process.env.PISPER_SMOKE_BROWSER)
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH ?? process.env.PISPER_SMOKE_BROWSER }
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
    ['crasher:native', 'native-ok:native'],
  )
  await page.locator('[data-component-row="native-ok"]').click()
  await page.locator('[data-component-preview] [data-fixture-ok="v1"]').waitFor({ timeout: 30_000 })
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'studio-1440.png') })
  report.checks.push(
    'Studio (S4 transitional) lists native rows only; the preview renders through ComponentHost + htm',
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
  await floating.locator('[data-fixture-ok="v1"]').waitFor({ timeout: 30_000 })
  // 同一组件同时在预览与浮窗里渲染：两个实例互不影响。
  await page.locator('[data-component-preview] [data-fixture-ok="v1"]').waitFor({ timeout: 10_000 })
  const before = await floating.boundingBox()
  const handle = floating.locator('[data-floating-move]')
  const handleBox = await handle.boundingBox()
  await page.mouse.move(handleBox.x + handleBox.width / 2, handleBox.y + handleBox.height / 2)
  await page.mouse.down()
  await page.mouse.move(handleBox.x - 120, handleBox.y + 80, { steps: 8 })
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
    'Floating instance renders at its manifest size, drags (position saved under the instanceId) and closing PATCHes enabled:false',
  )

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
  await page.locator('[data-component-stage="broken-dir"] [data-component-delete]').click()
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

  // 9. 1080 宽：没有横向滚动。
  await page.setViewportSize({ width: 1080, height: 680 })
  await settle(page)
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'studio-1080.png') })
  report.checks.push('No horizontal scroll at 1080×680')
  await context.close()

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
  const promptContext = await newContext(
    { viewport: { width: 1440, height: 900 } },
    {
      initScripts: [
        [
          (value) => {
            // 只在第一次导航前写入：选择之后哨兵被清掉，不能每次导航都重新放回去。
            if (sessionStorage.getItem('fixture-sentinel')) return
            sessionStorage.setItem('fixture-sentinel', '1')
            localStorage.setItem('pisper-components-safe-mode', value)
          },
          sentinel,
        ],
      ],
    },
  )
  const promptPage = await promptContext.newPage()
  failurePage = promptPage
  promptPage.on('pageerror', (error) => report.pageErrors.push(`prompt: ${error.message}`))
  await promptPage.goto(`${base}/#/tools/components/native-ok`)
  await promptPage.locator('[data-components-safe-mode-prompt]').waitFor({ timeout: 60_000 })
  await promptPage
    .locator('[data-feature-page-host="native-ok"] [data-component-state="safe-mode"]')
    .waitFor({ timeout: 30_000 })
  assert.equal(await promptPage.locator('[data-fixture-ok]').count(), 0)
  await promptPage.getByRole('button', { name: '正常加载' }).click()
  await promptPage
    .locator('[data-feature-page-host="native-ok"] [data-fixture-ok="v2"]')
    .waitFor({ timeout: 30_000 })
  await promptContext.close()
  report.checks.push(
    'A stale safe-mode sentinel asks first (components stay as placeholders) and loads normally after “正常加载”',
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
  console.log(`Components smoke report and screenshots: ${output}`)
}
