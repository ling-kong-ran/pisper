import { verifyPageTemplates } from './smoke-page-template-checks.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { writeFile, mkdtemp, mkdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { chromium } from 'playwright-core'
import { verifyRemoteWorkspaceSettings } from './smoke-pisper-remote-checks.mjs'
import { verifyMemoryLifecycle } from './smoke-pisper-memory-checks.mjs'
import { verifyPisperIteration } from './smoke-pisper-iteration-checks.mjs'
import { verifyFirstRunSetup } from './smoke-pisper-first-run-checks.mjs'
import { verifySideChat } from './smoke-pisper-side-chat-checks.mjs'
import { verifySessionTreeLifecycle } from './smoke-pisper-tree-checks.mjs'
import { DEFAULT_BRANCH } from '../shared/app-update.mjs'
import { fetchIdempotent } from './fixtures/route-fetch.mjs'
import { SMOKE_BROWSER } from './fixtures/smoke-browser.mjs'
// Every held test response has a bounded wait, including failure-injection paths.
async function within(promise, label, ms = 30000) {
  let timer
  try {
    return await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`Timed out: ${label}`)), ms)
      }),
    ])
  } finally {
    clearTimeout(timer)
  }
}
// 前面步骤（改名保存、快速设置、删除模型）发出的通知在应用里排队，每条显示 2.8 秒后才换下一条。
// 每条新通知都是 Radix 层栈里最新、也就是最上层的可关闭层：选择器开着时冒出来的通知会先吃掉 Esc
// （Radix 通知关掉自己但不 preventDefault），选择器那一下 Esc 就落空，偶发“列表框没关”。
// 检查选择器前逐条点掉排队的通知直到队列清空；点通知不会关掉浮层（通知视口是 Radix 的 Branch）。
async function dismissQueuedToasts(page) {
  const close = page.locator('[data-slot="toast"] [data-slot="toast-close"]')
  for (let index = 0; index < 20 && (await close.count()); index++) await close.first().click()
  assert.equal(await close.count(), 0, 'queued toasts are cleared before the picker check')
}
// 思考等级和模型名称共用一个选择器，上下两行、键盘和触发器间留白都可操作。
async function verifyUnifiedModelPicker(page) {
  const panel = page.locator('.model-effort-popover')
  const picker = panel.getByRole('combobox', { name: '当前会话模型' })
  await dismissQueuedToasts(page)
  assert.equal(await panel.count(), 1, 'dismissing queued toasts keeps the model popover open')
  const closePicker = async (trigger) => {
    await page.evaluate(() => {
      window.__pickerEscapeEvents = []
      const capture = (event) => {
        if (event.key !== 'Escape') return
        const state = {
          targetRole: event.target?.getAttribute?.('role'),
          targetSlot: event.target?.getAttribute?.('data-slot'),
          preventedAtCapture: event.defaultPrevented,
          layersAtCapture: window.__pisperLayerDump?.(),
        }
        window.__pickerEscapeEvents.push(state)
        queueMicrotask(() => {
          state.preventedAfterDispatch = event.defaultPrevented
        })
      }
      document.addEventListener('keydown', capture, { capture: true, once: true })
    })
    await page.keyboard.press('Escape')
    try {
      await page.getByRole('listbox').waitFor({ state: 'hidden' })
    } catch (error) {
      const diagnostic = await page.evaluate(() => ({
        events: window.__pickerEscapeEvents,
        layersAfterWait: window.__pisperLayerDump?.(),
        activeElement: document.activeElement?.outerHTML,
        overlays: Array.from(
          document.querySelectorAll(
            '[data-slot="dialog-content"],[data-slot="popover-content"],[role="listbox"],[role="menu"]',
          ),
        ).map((element) => ({
          slot: element.getAttribute('data-slot'),
          role: element.getAttribute('role'),
          state: element.getAttribute('data-state'),
          html: element.outerHTML,
        })),
      }))
      await page.keyboard.press('Escape')
      diagnostic.retryAfterFailureClosed = await page
        .getByRole('listbox')
        .waitFor({ state: 'hidden', timeout: 1000 })
        .then(
          () => true,
          () => false,
        )
      await writeFile(
        join(output, 'model-picker-escape-diagnostic.json'),
        JSON.stringify({ trigger, ...diagnostic }, null, 2),
      )
      throw error
    }
  }
  assert.equal(await panel.getByRole('combobox').count(), 1)
  assert.equal(await picker.isEnabled(), true)
  const bounds = await picker.boundingBox()
  assert.ok(bounds && bounds.height >= 48 && bounds.height <= 60)
  for (const fraction of [0.25, 0.5, 0.75]) {
    await picker.click({ position: { x: bounds.width / 2, y: bounds.height * fraction } })
    await page.getByRole('listbox').waitFor()
    await closePicker(`pointer-${fraction}`)
    assert.equal(await picker.evaluate((el) => el === document.activeElement), true)
  }
  await picker.press('Enter')
  await page.getByRole('listbox').waitFor()
  await closePicker('keyboard')
  await picker.press('Tab')
  assert.equal(
    await page
      .getByRole('slider', { name: '当前思考等级' })
      .evaluate((el) => el === document.activeElement),
    true,
  )
}
// 始终使用全新的临时后端，不连接已安装应用，也不读取真实密钥。
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = await mkdtemp(join(tmpdir(), 'pisper-ui-'))
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
await mkdir(workspace)
// 所有 CLI 扫描限于空的隔离 home，页面路由 mock 之外再提供后端安全边界。
const fixtureHome = join(output, 'home')
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
const runtime = await createPisperRuntime({
  root,
  frontendRoot: join(root, 'dist'),
  runtimeCwd: workspace,
  dataDir,
  production: true,
  port: 0,
  host: '127.0.0.1',
  remote: { enabled: false },
})
const base = runtime.url
const provider = 'pi-control-ui-smoke'
const alternate = provider + '-alt'
const model = 'pi-ui-fixture'
let finishDeferredResponse = () => {
  throw new Error('deferred fixture not started')
}
let activeDeferredFixture = null
const report = {
  backend: base,
  fixture: 'loopback-only OpenAI-compatible SSE, no external model',
  status: 'running',
  mockedServices: ['provider discovery', 'local CLI auto-import', 'GitHub update check'],
  checks: [],
  requests: 0,
  stopConnectionClosed: false,
  pageErrors: [],
  failedApi: [],
  expectedFailedApi: [],
}
let page, browser, sessionId
const fixture = createServer(async (req, res) => {
  try {
    if (req.method === 'GET') {
      // Each connection exposes its own catalog, just like its configured endpoint.
      const suffix = new URL(req.url, 'http://fixture').pathname
        .split('/')[1]
        .slice(provider.length)
      const discoveredModel = model + suffix
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          object: 'list',
          data: [{ id: discoveredModel, object: 'model', owned_by: 'local-test' }],
        }),
      )
      return
    }
    let raw = ''
    for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw)
    const responseModel = body.model
    report.requests++
    const last = body.messages?.findLast((x) => x.role === 'user')?.content
    const slow = JSON.stringify(last).includes('stop-test')
    const deferred = JSON.stringify(last).includes('defer-model-test')
    const content = slow ? '正在生成停止测试' : '流式回复：验收通过'
    if (!body.stream) {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          id: 'local-test',
          object: 'chat.completion',
          model: responseModel,
          choices: [{ index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' }],
          usage: { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 },
        }),
      )
      return
    }
    res.writeHead(200, {
      'content-type': 'text/event-stream; charset=utf-8',
      'cache-control': 'no-cache',
    })
    const chunk = (delta, finish_reason = null, usage) =>
      res.write(
        `data: ${JSON.stringify({ id: 'local-test', object: 'chat.completion.chunk', created: Math.floor(Date.now() / 1000), model: responseModel, choices: [{ index: 0, delta, finish_reason }], ...(usage ? { usage } : {}) })}\n\n`,
      )
    chunk({ role: 'assistant', content: slow ? content : '流式回复：' })
    if (deferred) {
      const held = {
        requestNumber: report.requests,
        model: responseModel,
        failureCase: JSON.stringify(last).includes('defer-model-test failure'),
        startedAt: Date.now(),
        finishedAt: null,
        closedAt: null,
      }
      activeDeferredFixture = held
      chunk({ content: '正在生成模型切换测试' })
      finishDeferredResponse = () => {
        held.finishedAt = Date.now()
        chunk({}, 'stop')
        res.end('data: [DONE]\n\n')
      }
      res.on('close', () => {
        held.closedAt = Date.now()
      })
    } else if (slow) {
      const timer = setInterval(() => {
        if (!res.destroyed) chunk({ content: ' …' })
      }, 500)
      res.on('close', () => {
        clearInterval(timer)
        report.stopConnectionClosed = true
      })
    } else {
      setTimeout(() => {
        if (!res.destroyed) {
          chunk({ content: '验收通过' })
          chunk({}, 'stop', { prompt_tokens: 12, completion_tokens: 8, total_tokens: 20 })
          res.end('data: [DONE]\n\n')
        }
      }, 1200)
    }
  } catch (error) {
    if (!res.headersSent) res.writeHead(500)
    res.end(String(error))
  }
})
await new Promise((resolve) => fixture.listen(0, '127.0.0.1', resolve))
async function api(path, method = 'GET', data) {
  const r = await fetch(base + path, {
    method,
    headers: { Origin: base, 'Content-Type': 'application/json' },
    body: data ? JSON.stringify(data) : undefined,
  })
  const text = await r.text()
  assert.ok(r.ok, `${method} ${path}: ${r.status} ${text.slice(0, 500)}`)
  return text ? JSON.parse(text) : null
}
try {
  await api('/api/providers', 'POST', {
    id: provider,
    name: 'PI Local UI Test',
    providerType: 'chat',
    api: 'openai-completions',
    baseUrl: `http://127.0.0.1:${fixture.address().port}/${provider}/v1`,
    apiKey: 'local-test-only-not-a-secret',
    model,
    modelKind: 'chat',
    enabled: true,
    reasoning: true,
    thinkingLevels: ['off', 'low', 'medium', 'high'],
  })
  await api('/api/providers', 'POST', {
    id: alternate,
    name: 'PI Alternate UI Test',
    providerType: 'chat',
    api: 'openai-completions',
    baseUrl: `http://127.0.0.1:${fixture.address().port}/${alternate}/v1`,
    apiKey: 'local-test-only-not-a-secret',
    model: model + '-alt',
    modelKind: 'chat',
    enabled: true,
    reasoning: true,
    thinkingLevels: ['off', 'low', 'medium', 'high'],
  })
  for (const [suffix, reasoning, levels] of [
    ['-plain', false, []],
    ['-fixed', true, ['off']],
  ]) {
    await api('/api/providers', 'POST', {
      id: provider + suffix,
      name: `PI ${suffix} UI Test`,
      providerType: 'chat',
      api: 'openai-completions',
      baseUrl: `http://127.0.0.1:${fixture.address().port}/${provider + suffix}/v1`,
      apiKey: 'local-test-only-not-a-secret',
      model: model + suffix,
      modelKind: 'chat',
      enabled: true,
      reasoning,
      ...(reasoning ? { thinkingLevels: levels } : {}),
    })
  }
  const config = await api('/api/config', 'PUT', { provider, setAsDefault: true })
  assert.equal(config.provider, provider)
  report.checks.push('real provider configuration API with isolated loopback fixture')
  browser = await chromium.launch({
    headless: true,
    ...(SMOKE_BROWSER
      ? { executablePath: SMOKE_BROWSER }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  const context = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    locale: 'zh-CN',
  })
  page = await context.newPage()
  await page.clock.install()
  if (process.env.PISPER_UI_DEBUG_LAYERS === '1') {
    // 只在隔离故障诊断中暴露依赖自己的层栈，不改变注册/事件时序或产品文件。
    await page.route('**/assets/*.js', async (route) => {
      const response = await fetchIdempotent(route)
      let body = await response.text()
      const match = body.match(
        /(\w+)=\w+\.createContext\(\{layers:new Set,layersWithOutsidePointerEventsDisabled:new Set,branches:new Set,dismissableSurfaces:new Set\}\)/,
      )
      if (match)
        body += `\nglobalThis.__pisperLayerDump = () => [...${match[1]}._currentValue.layers].map(element => ({role:element.getAttribute('role'),slot:element.getAttribute('data-slot'),state:element.getAttribute('data-state'),connected:element.isConnected}));`
      await route.fulfill({ response, body })
    })
  }
  page.on('pageerror', (e) => report.pageErrors.push(e.message))
  page.on('response', (r) => {
    if (r.url().includes('/api/') && r.status() >= 400)
      report.failedApi.push({ path: new URL(r.url()).pathname, status: r.status() })
  })
  page.on('request', (r) => {
    if (r.method() === 'POST' && new URL(r.url()).pathname === '/api/chat')
      sessionId = r.postDataJSON().sessionId
  })
  // 网络发现与 GitHub 更新不是本地界面验收目标，固定结果避免依赖网络及未推送的 HEAD。
  await page.route('**/api/providers/discovery', (r) =>
    r.fulfill({ json: { providers: [], errors: [] } }),
  )
  await page.route('**/api/providers/import-local', async (r) =>
    r.fulfill({
      json: {
        config: await api('/api/config'),
        discovery: { providers: [], errors: [] },
        imported: [],
        skipped: [],
      },
    }),
  )
  await page.route('**/api/app-update*', (r) =>
    r.fulfill({
      json: {
        state: 'current',
        currentVersion: '0.0.0',
        currentCommit: '0'.repeat(40),
        availableCommit: '0'.repeat(40),
        behindBy: 0,
        branch: DEFAULT_BRANCH,
        notes: '',
        releaseDate: null,
        releaseUrl: '',
        canDownload: false,
        checkedAt: Date.now(),
        message: 'Local UI fixture: GitHub update checks are verified separately.',
      },
    }),
  )
  await page.addInitScript(() => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      if (!localStorage.getItem('pisper-ui')) localStorage.setItem('pisper-theme', 'light')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
    } catch {}
  })
  // 全部代理（A3 取代泳道与启动台，规格 3.1）：空的全部代理只有一句“还没有代理在运行”+“新任务”，
  // 没有任务输入框和“试试”建议；“新任务”打开派发对话框，Esc 关闭、什么都不派发。
  await page.goto(base + '/#/board')
  const boardRoot = page.locator('[data-board-root]')
  await boardRoot.waitFor({ timeout: 60000 })
  await boardRoot.getByText('还没有代理在运行', { exact: true }).waitFor({ timeout: 60000 })
  assert.equal(await page.locator('[data-launch-input]').count(), 0)
  assert.equal(await page.getByRole('region', { name: '试试这些任务' }).count(), 0)
  await boardRoot.getByRole('button', { name: '新任务', exact: true }).click()
  const dispatchDialog = page.getByRole('dialog', { name: '新任务', exact: true })
  await dispatchDialog.waitFor()
  await page.keyboard.press('Escape')
  await dispatchDialog.waitFor({ state: 'hidden' })
  assert.equal(report.requests, 0)
  report.checks.push(
    'Empty 全部代理 shows only “还没有代理在运行” and 新任务 (no launch console, no suggestions); 新任务 opens the dispatch dialog and Esc closes it without sending',
  )
  // 规格 v2：没有标题栏，一级入口在侧栏（全部代理 / 功能 / 设置）；旧的“组件”标签归“功能”。
  const titlebar = page.locator('[data-shell-sidebar]')
  const SIDEBAR_ENTRIES = {
    board: 'board',
    more: 'features',
    components: 'features',
    settings: 'settings',
  }
  const shellTab = (tab) => titlebar.locator(`[data-sidebar-entry="${SIDEBAR_ENTRIES[tab]}"]`)
  // 侧栏分组计数（取代标题栏舰队计量，所有页面常驻）：运行中与需要你两组标题上的数字。
  const fleet = (running, attention = 0) => ({
    waitFor: (options) =>
      page.waitForFunction(
        (expected) =>
          Object.entries(expected).every(
            ([group, count]) =>
              document.querySelector(`[data-sidebar-group="${group}"] [data-sidebar-count]`)
                ?.textContent === String(count),
          ),
        { running, attention },
        options,
      ),
  })
  assert.deepEqual(
    await titlebar
      .locator('[data-sidebar-entry]')
      .evaluateAll((entries) => entries.map((entry) => entry.getAttribute('data-sidebar-entry'))),
    ['newTask', 'search', 'board', 'features', 'settings'],
  )
  await shellTab('more').click()
  await page.locator('[data-more-tile="page:workflows"]').waitFor()
  // 会话页：/board/new 建好会话后以 replace 换成真实 id；会话页沿用单会话视图与欢迎语。
  await page.goto(base + '/#/board/new')
  await page.waitForFunction(() => /^#\/board\/(?!new$)[^/]+$/.test(location.hash))
  const sessionHash = new URL(page.url()).hash
  await page.getByTestId('workbench-greeting').getByRole('heading').waitFor()
  // 看板启动台的输入框也叫“任务描述”；限定在会话输入区，避免换页瞬间写进将要卸载的启动台。
  const prompt = page.locator('.focus-composer').getByRole('textbox', { name: '任务描述' })
  await prompt.waitFor()
  assert.equal(await page.locator('[data-board-root]').count(), 0)
  const sendBox = await page.getByRole('button', { name: '发送消息', exact: true }).boundingBox()
  assert.equal(sendBox.width, 32)
  assert.equal(sendBox.height, 32)
  const catalogReady = Promise.withResolvers()
  const catalogRelease = Promise.withResolvers()
  await page.route('**/api/providers/models/refresh', async (route) => {
    const response = await route.fetch()
    catalogReady.resolve()
    await within(catalogRelease.promise, 'release stale catalog response')
    await route.fulfill({ response })
  })
  await prompt.fill('return home keeps this draft')
  await shellTab('settings').click()
  await page.locator('[data-model-provider-list]').waitFor()
  // A6：设置分区的页标题是目录里的短名（主区不再重复“设置 / 模型配置”面包屑与大标题）。
  await page.getByRole('heading', { name: '模型', level: 1, exact: true }).waitFor()
  assert.equal(await page.getByText('模型配置', { exact: true }).count(), 0)
  assert.match(page.url(), /config\/models/)
  // A6 评审返修：提供方是 FeatureRow 行（效果图 desktop-settings-light.png），展开一行原位编辑；
  // 行的折叠键读屏名称就是提供方名称（aria-labelledby 指向行名），展开状态用 aria-expanded。
  const connectionList = page.locator('[data-model-provider-list]')
  const providerToggles = connectionList.locator('[data-feature-row-toggle]')
  const visibleProviders = (await api('/api/config')).providers.filter(
    (item) => item.configured || item.custom,
  )
  assert.deepEqual(
    await providerToggles.evaluateAll((nodes) =>
      nodes.map(
        (node) => document.getElementById(node.getAttribute('aria-labelledby')).textContent,
      ),
    ),
    visibleProviders.map((item) => item.name),
  )
  // 进入页面时全部收起（效果图），默认值三行在提供方分区上面。
  assert.deepEqual(
    await providerToggles.evaluateAll((nodes) =>
      nodes.map((node) => node.getAttribute('aria-expanded')),
    ),
    visibleProviders.map(() => 'false'),
  )
  await page
    .locator('[data-config-target="models-runtime-policy"]')
    .getByText('默认模型', { exact: true })
    .waitFor()
  const providerRow = (name) =>
    connectionList.locator('[data-feature-row]').filter({
      has: page.getByRole('button', { name, exact: true }),
    })
  const defaultConnection = connectionList.getByRole('button', {
    name: 'PI Local UI Test',
    exact: true,
  })
  await providerRow('PI Local UI Test').locator('[data-provider-default]').waitFor()
  await connectionList.getByRole('button', { name: 'PI Alternate UI Test', exact: true }).click()
  assert.equal(
    await connectionList
      .getByRole('button', { name: 'PI Alternate UI Test', exact: true })
      .getAttribute('aria-expanded'),
    'true',
  )
  assert.equal(
    await providerRow('PI Local UI Test').locator('[data-provider-default]').isVisible(),
    true,
  )
  assert.equal(await defaultConnection.getAttribute('aria-expanded'), 'false')
  assert.equal(await connectionList.locator('[data-provider-default]').count(), 1)
  await defaultConnection.click()
  assert.equal(await defaultConnection.getAttribute('aria-expanded'), 'true')
  // 同一时刻只展开一行。
  assert.equal(await connectionList.locator('[data-provider-connection-editor]').count(), 1)
  report.checks.push(
    'Provider rows start collapsed under the defaults rows; the default marker stays on the default provider while another row is expanded; one row open at a time',
  )
  // “快速设置”在页内工具行（A6，共享页头退役）。
  const quickSetup = page.locator('main [data-settings-quick-setup]').getByText('快速设置', {
    exact: true,
  })
  assert.equal(await quickSetup.count(), 1)
  assert.equal(await page.getByRole('button', { name: '添加供应商', exact: true }).count(), 0)
  assert.equal(await page.getByRole('button', { name: '快速配置', exact: true }).count(), 0)
  assert.equal(await page.getByText('预置服务', { exact: true }).count(), 0)
  const formFont = await page
    .locator('[data-provider-connection-editor] input')
    .first()
    .evaluate((el) => getComputedStyle(el).fontSize)
  // UI 表单辅助字号按已冻结的 sm 角色为13px；根rem仍保持16px。
  assert.equal(formFont, '13px')
  assert.equal(
    await page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
    '16px',
  )
  await page.screenshot({ path: join(output, 'model-settings-refined.png') })
  await within(catalogReady.promise, 'catalog refresh captures old configuration')
  const configForm = page.locator('[data-provider-connection-editor]')
  const nameInput = configForm.getByLabel('显示名称', { exact: true })
  const originalProviderName = await nameInput.inputValue()
  const savedProviderName = 'PI UI Saved During Catalog Refresh'
  await nameInput.fill(savedProviderName)
  await configForm.getByRole('button', { name: '保存修改', exact: true }).click()
  await connectionList.getByRole('button', { name: savedProviderName, exact: true }).waitFor()
  const staleCatalogFinished = page.waitForEvent('requestfinished', {
    predicate: (request) => request.url().endsWith('/api/providers/models/refresh'),
  })
  catalogRelease.resolve()
  await staleCatalogFinished
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(
    await nameInput.inputValue(),
    savedProviderName,
    'late catalog response must not overwrite a saved connection',
  )
  assert.equal(
    await connectionList
      .locator('[data-feature-row][data-expanded="true"] [data-feature-row-toggle]')
      .evaluate(
        (node) => document.getElementById(node.getAttribute('aria-labelledby')).textContent,
      ),
    savedProviderName,
    'late catalog must preserve the saved provider name on the expanded row',
  )
  await page.unroute('**/api/providers/models/refresh')
  await nameInput.fill(originalProviderName)
  await configForm.getByRole('button', { name: '保存修改', exact: true }).click()
  await connectionList.getByRole('button', { name: originalProviderName, exact: true }).waitFor()
  report.checks.push(
    'late background catalog refresh cannot overwrite a newly saved provider configuration',
  )

  for (const outcome of ['retry', 'cancel']) {
    const connectionName = `PI Batch ${outcome}`
    const batchRoute = /\/api\/providers\/custom-[0-9a-f]{32}\/models\/batch$/
    let connectionId = ''
    let createCount = 0
    const trackCreates = (request) => {
      if (new URL(request.url()).pathname === '/api/providers' && request.method() === 'POST') {
        createCount += 1
        connectionId = request.postDataJSON().id
      }
    }
    page.on('request', trackCreates)
    await page.route(batchRoute, (route) => {
      const batchPath = `/api/providers/${connectionId}/models/batch`
      assert.equal(new URL(route.request().url()).pathname, batchPath)
      report.expectedFailedApi.push({ path: batchPath, status: 503 })
      return route.fulfill({
        status: 503,
        json: { error: `PI fixture rejected batch ${outcome}` },
      })
    })
    await quickSetup.click()
    const editor = page.getByRole('dialog')
    await editor
      .getByLabel('Base URL', { exact: true })
      .fill(`http://127.0.0.1:${fixture.address().port}/${provider}/v1`)
    await editor.getByRole('button', { name: '下一步', exact: true }).click()
    await editor.getByRole('combobox', { name: 'API 协议', exact: true }).click()
    await page.getByRole('option', { name: 'OpenAI Chat Completions', exact: true }).click()
    await editor.getByRole('button', { name: '下一步', exact: true }).click()
    await editor.getByLabel('显示名称', { exact: true }).fill(connectionName)
    await editor.getByLabel('API Key', { exact: true }).fill('local-ui-fixture-key')
    const primaryModel = editor.getByLabel('模型 ID（手动输入）', { exact: true })
    if (outcome === 'retry') {
      const discovered = page.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === '/api/providers/models/discover-connection' &&
          response.request().method() === 'POST',
      )
      await editor.getByRole('button', { name: '获取模型', exact: true }).click()
      assert.equal((await discovered).status(), 200)
      await editor.getByRole('button', { name: model, exact: true }).click()
      assert.equal(await primaryModel.inputValue(), model)
    } else {
      const discoveryPath = '/api/providers/models/discover-connection'
      const discoveryError = 'PI fixture model discovery unavailable'
      await page.route(`**${discoveryPath}`, (route) =>
        route.fulfill({ status: 503, json: { error: discoveryError } }),
      )
      report.expectedFailedApi.push({ path: discoveryPath, status: 503 })
      await editor.getByRole('button', { name: '获取模型', exact: true }).click()
      await editor.getByText(discoveryError, { exact: true }).waitFor()
      await page.unroute(`**${discoveryPath}`)
      await primaryModel.fill(model)
    }
    assert.equal(createCount, 0, 'model discovery must not persist a provider')
    await editor
      .getByRole('textbox', { name: '追加模型 ID（可选）', exact: true })
      .fill(`${model}-extra`)
    await editor.getByRole('button', { name: '追加模型 ID（可选）', exact: true }).click()
    await editor.getByRole('button', { name: '保存修改', exact: true }).click()
    await editor.getByText(`PI fixture rejected batch ${outcome}`, { exact: true }).waitFor()
    assert.match(connectionId, /^custom-[0-9a-f]{32}$/)
    assert.equal(
      await connectionList
        .getByRole('button', { name: connectionName, exact: true, includeHidden: true })
        .count(),
      1,
    )
    await page.unroute(batchRoute)
    if (outcome === 'retry') {
      await editor.getByRole('button', { name: '保存修改', exact: true }).click()
    } else {
      await editor.getByRole('button', { name: '关闭对话框', exact: true }).click()
    }
    await editor.waitFor({ state: 'hidden' })
    await connectionList.getByRole('button', { name: connectionName, exact: true }).waitFor()
    const savedProvider = (await api('/api/config')).providers.find(
      (item) => item.id === connectionId,
    )
    assert.ok(savedProvider, 'committed connection survives partial failure')
    assert.equal(savedProvider.api, 'openai-completions')
    assert.equal(savedProvider.defaultModel, model)
    assert.ok(savedProvider.models.some((item) => item.id === model))
    assert.equal(
      savedProvider.models.some((item) => item.id === `${model}-extra`),
      outcome === 'retry',
    )
    assert.equal(createCount, 1, 'retry must never issue a second create')
    page.off('request', trackCreates)
    assert.equal(
      await connectionList
        .getByRole('button', { name: connectionName, exact: true })
        .getAttribute('aria-expanded'),
      'true',
      'a committed connection stays expanded after retry or close',
    )
    const deleteModel = page.getByRole('button', { name: `删除模型 ${model}`, exact: true })
    await deleteModel.click()
    const deleteDialog = page.getByRole('dialog', { name: '删除模型', exact: true })
    await deleteDialog.getByRole('button', { name: '取消', exact: true }).click()
    assert.ok(
      (await api('/api/config')).providers
        .find((item) => item.id === connectionId)
        .models.some((item) => item.id === model),
      'cancel keeps the model',
    )
    await deleteModel.click()
    await deleteDialog.getByRole('button', { name: '删除', exact: true }).click()
    await deleteDialog.waitFor({ state: 'hidden' })
    await deleteModel.waitFor({ state: 'hidden' })
    const remainingProvider = (await api('/api/config')).providers.find(
      (item) => item.id === connectionId,
    )
    assert.ok(remainingProvider.configured, 'deleting a model retains the connection and key')
    assert.ok(!remainingProvider.models.some((item) => item.id === model))
    assert.equal(remainingProvider.models.length, outcome === 'retry' ? 1 : 0)
    await connectionList.getByRole('button', { name: connectionName, exact: true }).waitFor()
    await connectionList.getByRole('button', { name: originalProviderName, exact: true }).click()
    await api(`/api/providers/${connectionId}`, 'DELETE')
  }
  report.checks.push(
    'Quick setup discovers models or accepts manual IDs after discovery failure; partial creation stays visible and expanded, retry creates only once, and close retains the saved connection',
    'Model deletion supports cancel and confirmation, removes only the selected model, and retains an empty provider connection after the last model is deleted',
  )

  await page
    .getByRole('navigation', { name: '设置导航', exact: true })
    // 设置目录与页标题都用短名（规格 2.6，A6 起主区页标题也是“界面”）。
    .getByRole('button', { name: '界面', exact: true })
    .click()
  await page.waitForURL('**/#/config/interface')
  // A3 设置整页：侧栏整块换成设置目录（规格 2.3），当前分区在目录里点亮。
  await page
    .locator(
      '[data-shell-sidebar="settings"] [data-settings-section="interface"][aria-current="page"]',
    )
    .waitFor()
  // 空会话不上看板；经会话链接回到会话页（同一文档内的哈希跳转，不刷新），草稿仍在。
  await page.goto(base + '/' + sessionHash)
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'return home keeps this draft')
  await prompt.fill('')
  report.checks.push(
    '32px send button; sidebar entries board/features/settings with workflows on 功能; Settings take over the sidebar with the section directory and returning through the session link restores the conversation draft',
    'Provider navigation shows configured or custom connections with one Quick setup entry and no preset catalog',
  )

  // 规格 v2 第 4.3 节：工具行固定为 [+] [盾牌芯片] … [模型芯片] [发送]，可配置的工具条与收纳区退役。
  const composerToolbarOrder = () =>
    page
      .locator('[data-composer-toolbar]')
      .evaluate((bar) =>
        [
          ...bar.querySelectorAll(
            '[data-composer-plus], .composer-mode-chip, .model-effort-pill, .send-button',
          ),
        ].map((node) =>
          node.matches('[data-composer-plus]')
            ? 'plus'
            : node.matches('.composer-mode-chip')
              ? 'mode'
              : node.matches('.model-effort-pill')
                ? 'model'
                : 'send',
        ),
      )
  assert.deepEqual(await composerToolbarOrder(), ['plus', 'mode', 'model', 'send'])
  assert.equal(
    await page.locator('.composer-mode-chip').getAttribute('aria-label'),
    '审批与执行方式 · 审批后写入',
    'no elevated default: the shield chip starts on approval-required with Plan',
  )
  for (const retired of [
    '[data-composer-tool-id]',
    '.composer-tools-trigger',
    '[data-composer-hint]',
    '.composer-details',
    '.focus-composer-status',
  ])
    assert.equal(await page.locator(retired).count(), 0, `${retired} is retired`)
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page
    .locator('.model-effort-model')
    .getByRole('combobox', { name: '当前会话模型' })
    .waitFor()
  await page.waitForFunction(
    () =>
      Math.abs(
        document.querySelector('.model-effort-popover').getBoundingClientRect().width - 272,
      ) < 0.1,
  )
  assert.equal(
    await page
      .getByRole('slider', { name: '当前思考等级' })
      .evaluate((el) => getComputedStyle(el).height),
    '28px',
  )

  assert.equal(
    await page
      .locator('.model-effort-model')
      .getByRole('combobox', { name: '当前会话模型' })
      .isEnabled(),
    true,
  )
  await verifyUnifiedModelPicker(page)
  await page
    .locator('.model-effort-popover')
    .screenshot({ path: join(output, 'unified-model-picker-light.png') })
  report.checks.push(
    'One shared model trigger opens from its upper/lower rows and middle, with keyboard access and focus restored; Tab reaches the independent effort slider',
  )
  await page.locator('.model-effort-model').getByRole('combobox', { name: '当前会话模型' }).click()
  await page.getByRole('option', { name: /pi-ui-fixture-alt/ }).click()
  await page
    .locator('.model-effort-model')
    .getByRole('combobox', { name: '当前会话模型' })
    .filter({ hasText: 'pi-ui-fixture-alt' })
    .waitFor()
  // 模型名称先乐观更新；需要等保存结束后再操作暂时禁用的思考等级。
  await page.waitForFunction(
    () => document.querySelector('[aria-label="当前思考等级"]')?.disabled === false,
  )
  await page.getByRole('slider', { name: '当前思考等级' }).press('End')
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
      '深度',
  )
  await page.keyboard.press('Escape')
  await page.reload()
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page
    .locator('.model-effort-model')
    .getByRole('combobox', { name: '当前会话模型' })
    .filter({ hasText: 'pi-ui-fixture-alt' })
    .waitFor({ timeout: 30000 })
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
      '深度',
  )
  await page.keyboard.press('Escape')
  report.checks.push(
    'one model/reasoning panel updates both real session settings and persists after reload',
  )
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  for (const suffix of ['-plain', '-fixed']) {
    await page
      .locator('.model-effort-model')
      .getByRole('combobox', { name: '当前会话模型' })
      .click()
    await page.getByRole('option', { name: new RegExp(`pi-ui-fixture${suffix}`) }).click()
    await page
      .locator('.model-effort-model')
      .getByRole('combobox', { name: '当前会话模型' })
      .filter({ hasText: `pi-ui-fixture${suffix}` })
      .waitFor()
    await page.waitForFunction(
      () => !document.querySelector('[aria-label="当前会话模型"]')?.disabled,
    )
    // The release backend may normalize a non-reasoning model to a fixed ['off'] level.
    await page.waitForFunction(() => {
      const control = document.querySelector('[aria-label="当前思考等级"]')
      return !control || control.disabled
    })
  }
  await page.locator('.model-effort-model').getByRole('combobox', { name: '当前会话模型' }).click()
  await page.getByRole('option', { name: /pi-ui-fixture-alt/ }).click()
  await page.waitForFunction(() => !document.querySelector('[aria-label="当前思考等级"]')?.disabled)
  await page.getByRole('slider', { name: '当前思考等级' }).press('End')
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
      '深度',
  )
  await page.keyboard.press('Escape')
  report.checks.push(
    'non-reasoning and fixed-effort providers disable reasoning; returning to a supported model restores editing',
  )
  // 盾牌芯片（规格 v2 第 2.5 节）：审批模式与执行方式（Plan / Goal / Team）在同一个弹层里；
  // 非 Plan 时芯片前面写出模式名。⇧Tab 在输入框里循环切换执行方式。
  const modeChip = (label) =>
    page.getByRole('button', { name: `审批与执行方式 · ${label}`, exact: true })
  const modePopover = page.locator('.composer-mode-popover')
  await modeChip('审批后写入').click()
  await modePopover.getByRole('radio', { name: /^Goal 模式/ }).click()
  await modeChip('Goal · 审批后写入').waitFor()
  await page.keyboard.press('Escape')
  await page.reload()
  await modeChip('Goal · 审批后写入').click()
  await modePopover.getByRole('radio', { name: /^Plan 模式/ }).click()
  await modeChip('审批后写入').waitFor()
  await prompt.focus()
  await prompt.press('Shift+Tab')
  await modeChip('Goal · 审批后写入').waitFor()
  assert.equal(
    await page.evaluate(() => document.activeElement?.matches('.focus-composer textarea')),
    true,
    'Shift+Tab cycles the run mode without moving focus out of the composer',
  )
  for (let step = 0; step < 3; step += 1) {
    if (await modeChip('审批后写入').count()) break
    await prompt.press('Shift+Tab')
  }
  await modeChip('审批后写入').waitFor()
  report.checks.push(
    'shield chip Plan/Goal selector changes backend mode, persists on reload, restores Plan; Shift+Tab in the composer cycles Plan → Goal (→ Team) → Plan',
  )
  await modeChip('审批后写入').click()
  await modePopover.getByRole('radio', { name: /^自动审批/ }).click()
  await modeChip('自动审批').waitFor()
  await page.reload()
  await modeChip('自动审批').waitFor()
  await modeChip('自动审批').click()
  await modePopover.getByRole('radio', { name: /^审批后写入/ }).click()
  await modeChip('审批后写入').waitFor()
  await modeChip('审批后写入').click()
  await modePopover.getByRole('radio', { name: /^完全访问/ }).click()
  const accessConfirmation = page.getByRole('dialog', { name: '启用完全访问', exact: true })
  await accessConfirmation.getByRole('button', { name: '取消', exact: true }).click()
  await modeChip('审批后写入').click()
  await modePopover.getByRole('radio', { name: /^完全访问/ }).click()
  await accessConfirmation.getByRole('button', { name: '启用完全访问', exact: true }).click()
  await modeChip('完全访问').waitFor()
  await page.reload()
  await modeChip('完全访问').click()
  await modePopover.getByRole('radio', { name: /^审批后写入/ }).click()
  await modeChip('审批后写入').waitFor()
  report.checks.push(
    'approval/auto/full access are real persisted settings; no elevated default; restored approval-required',
  )
  assert.equal(await page.getByRole('button', { name: '打开会话操作菜单', exact: true }).count(), 0)
  assert.equal(await page.getByRole('button', { name: /拆分到|关闭标签/ }).count(), 0)
  report.checks.push('no split-window or uncloseable pane entries in session menu')
  // 上下文用量从页头移到模型芯片弹层（规格 v2 第 2.5 节）：读数与自动压缩阈值都在这里。
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page.locator('.model-effort-popover [data-context-usage]').waitFor()
  assert.equal(
    await page.locator('.model-effort-popover [data-usage-row="cache"]').count(),
    1,
    'the model chip popover shows cache hits (or — when the provider does not report them)',
  )
  await page.keyboard.press('Escape')
  report.checks.push(
    'model chip popover carries context usage, session tokens and cache hits (the header ring and the details row are retired)',
  )
  const beforeIme = report.requests
  await prompt.fill('中文输入测试')
  await prompt.dispatchEvent('compositionstart')
  await prompt.dispatchEvent('keydown', {
    key: 'Enter',
    code: 'Enter',
    keyCode: 229,
    isComposing: true,
    bubbles: true,
  })
  assert.equal(report.requests, beforeIme)
  assert.equal(await prompt.inputValue(), '中文输入测试')
  await prompt.dispatchEvent('compositionend')
  await prompt.press('End')
  await prompt.press('Shift+Enter')
  assert.ok((await prompt.inputValue()).includes('\n'))
  assert.equal(report.requests, beforeIme)
  await prompt.fill('')
  report.checks.push('IME-composition Enter does not submit; Shift+Enter inserts a newline')

  await page.locator('[data-composer-plus]').click()
  const tools = await page
    .locator('[data-composer-plus-menu] [data-composer-plus-item]')
    .evaluateAll((nodes) => nodes.map((n) => n.dataset.composerPlusItem))
  for (const id of ['attachment', 'resource', 'voice'])
    assert.ok(tools.includes(id), `+ menu has ${id}: ${tools}`)
  console.log('TOOLS', tools)
  await page.keyboard.press('Escape')
  await page.locator('[data-composer-plus-menu]').waitFor({ state: 'detached' })
  assert.equal(
    await page.evaluate(() => document.activeElement?.hasAttribute('data-composer-plus')),
    true,
    'closing the + menu returns focus to +',
  )
  report.checks.push('+ menu exposes attachments, skills/tools/workflows and voice input (F8)')
  // 第 3 期：会话页右侧是检查器（固定标签 改动 / 终端 / 文件 / 预览），取代可关闭的上下文页标签；
  // 计划标签只在有计划、目标或团队时出现。标签条支持方向键与 Home / End。
  // 规格 v2 第 4.4 节：检查器默认关着，由顶栏的改动按钮（没有改动时只是检查器图标）按需滑入。
  const inspector = page.locator('[data-session-inspector]')
  assert.equal(await inspector.count(), 0, 'the inspector starts closed')
  await page.locator('[data-session-header] [data-inspector-toggle]').click()
  await inspector.waitFor()
  const inspectorTabIds = () =>
    inspector
      .locator('[data-inspector-tab-id]')
      .evaluateAll((tabs) => tabs.map((tab) => tab.getAttribute('data-inspector-tab-id')))
  assert.deepEqual(await inspectorTabIds(), ['changes', 'terminal', 'files', 'preview'])
  const inspectorTab = (id) => inspector.locator(`[data-inspector-tab-id="${id}"]`)
  await inspectorTab('changes').click()
  await inspectorTab('changes').focus()
  await page.keyboard.press('End')
  assert.equal(await inspectorTab('preview').getAttribute('aria-selected'), 'true')
  await inspector.locator('[data-inspector-preview]').waitFor()
  await page.keyboard.press('Home')
  await page.keyboard.press('ArrowRight')
  assert.equal(await inspectorTab('terminal').getAttribute('aria-selected'), 'true')
  await page.waitForFunction(() =>
    document.activeElement?.matches('[data-inspector-tab-id="terminal"]'),
  )
  await inspectorTab('changes').click()
  await page.getByRole('button', { name: '审批与执行方式 · 审批后写入', exact: true }).click()
  console.log('PERMISSIONS', await page.locator('.composer-mode-popover').ariaSnapshot())
  await page.keyboard.press('Escape')
  report.checks.push(
    'session inspector fixed tabs (changes / terminal / files / preview) with arrow, Home and End keyboard navigation',
  )
  // 活动栏的全局“资源”菜单随旧外壳退役；资料来源在检查器的“文件”标签里，
  // 已退役的工作区文件浏览器不会出现在固定标签或“+”说明里。
  await inspectorTab('files').click()
  await inspector.getByRole('region', { name: '来源', exact: true }).waitFor()
  assert.equal(await page.getByRole('tab', { name: '项目文件', exact: true }).count(), 0)
  await inspector.getByRole('button', { name: '添加组件标签', exact: true }).click()
  await page.locator('[data-slot="popover-content"]').waitFor()
  assert.equal(await page.getByRole('button', { name: '项目文件', exact: true }).count(), 0)
  await page.keyboard.press('Escape')
  await inspectorTab('changes').click()
  report.checks.push('The focused task shows its sources in the inspector files tab')
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 })
    await page.waitForFunction((expected) => window.innerWidth === expected, width)
    // 手机会话页是 56px 顶栏（规格 v2 第 7.2 节：☰ · 标题 ▾，侧聊与检查器都在标题 ▾ 的会话菜单里）；
    // 桌面会话顶栏（规格 4.1）左边是标题 ▾ 菜单，右端只有改动按钮（兼检查器开关）。
    const phoneLayout = width <= 650
    const taskHeader = page.locator(
      phoneLayout ? '[data-session-header="phone"]' : '[data-session-header]',
    )
    const headerControls = phoneLayout
      ? ['[data-phone-menu]', '[data-session-menu-trigger]']
      : ['[data-session-menu]', '[data-inspector-toggle]']
    await taskHeader.locator(headerControls.at(-1)).waitFor()
    await taskHeader.evaluate(async (header) => {
      header.getBoundingClientRect()
      await Promise.all(
        header
          .getAnimations({ subtree: true })
          .map((animation) => animation.finished.catch(() => {})),
      )
    })
    // 跨断点会重新挂载桌面/手机工作区；一次读取整组尺寸，避免跨调用混用两棵树。
    const headerMetrics = await taskHeader.evaluate((header, selectors) => {
      const rect = header.getBoundingClientRect()
      return {
        left: rect.left,
        right: rect.right,
        controls: selectors.map((selector) => {
          const button = header.querySelector(`button${selector}`)
          const bounds = button.getBoundingClientRect()
          return {
            name: button.getAttribute('aria-label') || button.textContent.trim(),
            left: bounds.left,
            right: bounds.right,
            width: bounds.width,
            height: bounds.height,
          }
        }),
      }
    }, headerControls)
    let previousRight = headerMetrics.left
    for (const bounds of headerMetrics.controls) {
      assert.ok(
        bounds.left >= previousRight && bounds.right <= headerMetrics.right + 1,
        `${bounds.name} stays inside the header without overlapping at ${width}px`,
      )
      if (phoneLayout)
        assert.ok(
          bounds.width >= 44 && bounds.height >= 44,
          `${bounds.name} has a mobile touch target: ${JSON.stringify(bounds)}`,
        )
      previousRight = bounds.right
    }
    // 桌面检查器为保存阅读位置按需建的资源页，不会让手机外壳落在只显示内容的视图上。
    if (width === 320)
      assert.equal(
        await prompt.isVisible(),
        true,
        'the phone layout opens on the conversation after the desktop inspector was used',
      )
    if (width === 390) {
      // 浏览器没有系统栏；模拟 Android 原生注入的真实 inset，检查最终位置而非 CSS 写法。
      await page.evaluate(() => {
        document.documentElement.style.setProperty('--pisper-safe-area-top', '24px')
        document.documentElement.style.setProperty('--pisper-safe-area-bottom', '16px')
      })
      await page.locator('[data-session-menu-trigger]').click()
      await page.locator('[data-session-menu-sheet] [data-inspector-toggle]').click()
      const content = page.locator('[data-inspector-sheet]')
      await content.waitFor()
      await content.evaluate((el) =>
        Promise.all(el.getAnimations().map((a) => a.finished.catch(() => {}))),
      )
      const bounds = await content.locator('[data-session-inspector]').evaluate((element) => {
        const rect = element.getBoundingClientRect()
        return { top: rect.top, bottom: rect.bottom, viewportHeight: innerHeight }
      })
      assert.ok(bounds.top >= 24 && bounds.bottom <= bounds.viewportHeight - 16)
      assert.equal(await page.evaluate(() => window.__pisperRequestBack()), true)
      await content.waitFor({ state: 'hidden' })
      await page.evaluate(() => {
        document.documentElement.style.removeProperty('--pisper-safe-area-top')
        document.documentElement.style.removeProperty('--pisper-safe-area-bottom')
      })
      report.checks.push(
        '390px phone inspector sheet (⋯ › 改动、终端、文件与预览) respects system safe areas and one native back closes it back to the conversation',
      )
    }
    // 固定工具行在每个宽度都完整可见；手机外壳下每个控件都是 44px 触控目标。
    assert.deepEqual(await composerToolbarOrder(), ['plus', 'mode', 'model', 'send'])
    const toolbarTargets = await page.locator('[data-composer-toolbar]').evaluate((bar) =>
      [
        ...bar.querySelectorAll(
          '[data-composer-plus], .composer-mode-chip, .model-effort-pill, .send-button',
        ),
      ].map((node) => {
        const rect = node.getBoundingClientRect()
        return { width: rect.width, height: rect.height, visible: node.checkVisibility() }
      }),
    )
    for (const target of toolbarTargets) {
      assert.ok(target.visible && target.width > 0, `toolbar control visible at ${width}`)
      if (phoneLayout)
        assert.ok(target.height >= 44, `toolbar control ≥44px at ${width}: ${target.height}`)
    }
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth),
      false,
      `document overflow at ${width}`,
    )
    const footer = await page
      .locator('.focus-composer-footer')
      .evaluate((node) => ({ width: node.clientWidth, scroll: node.scrollWidth }))
    assert.ok(
      footer.scroll <= footer.width + 1,
      `composer clips controls at ${width}: ${JSON.stringify(footer)}`,
    )
    await prompt.focus()
    if (width === 390) {
      const draft = await prompt.inputValue()
      await page.setViewportSize({ width, height: 500 })
      await page.waitForFunction(() => {
        const el = document.querySelector('.focus-composer textarea')
        return (
          el &&
          el.getBoundingClientRect().bottom <= innerHeight &&
          el.getBoundingClientRect().top >= 0
        )
      })
      assert.equal(await prompt.inputValue(), draft)
      await page.setViewportSize({ width, height: 900 })
      await page.waitForFunction(() => innerHeight === 900)
      assert.equal(await prompt.inputValue(), draft)
    }
    await page.screenshot({ path: join(output, `light-${width}.png`), animations: 'disabled' })
  }
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  // 规格 v2 第 2.5 节：主题不再在页头或会话菜单里（A4 起手机会话菜单也去掉了），入口是 ⌘K“切换主题”与设置 › 界面。
  // 循环顺序 浅色 → 跟随系统 → 深色 → 浅色；只有跟随系统时随操作系统变化；刷新后保留。
  await page.setViewportSize({ width: 1440, height: 900 })
  await page.waitForFunction(() => window.innerWidth === 1440)
  const storedTheme = () =>
    page.evaluate(() => JSON.parse(localStorage.getItem('pisper-ui') || '{}')?.state?.theme)
  const cycleTheme = async () => {
    await page.locator('[data-shell-sidebar]').getByRole('button', { name: /^搜索/ }).click()
    const palette = page.getByRole('combobox')
    await palette.waitFor()
    await palette.fill('切换主题')
    await page.getByRole('option', { name: /切换主题/ }).click()
    await palette.waitFor({ state: 'detached' })
  }
  await page.emulateMedia({ colorScheme: 'light' })
  for (let i = 0; i < 3 && (await storedTheme()) !== 'light'; i += 1) await cycleTheme()
  assert.equal(await storedTheme(), 'light')
  await cycleTheme()
  assert.equal(await storedTheme(), 'system')
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await page.emulateMedia({ colorScheme: 'light' })
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  await cycleTheme()
  assert.equal(await storedTheme(), 'dark')
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await cycleTheme()
  assert.equal(await storedTheme(), 'light')
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.waitForTimeout(200)
  assert.equal(
    await page.evaluate(() => document.documentElement.dataset.theme),
    'light',
    'an explicit light theme ignores the OS',
  )
  await cycleTheme()
  await cycleTheme()
  assert.equal(await storedTheme(), 'dark')
  await page.reload()
  await prompt.waitFor()
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  assert.equal(await storedTheme(), 'dark')
  await page.locator('[data-session-header] [data-session-menu]').waitFor()
  report.checks.push(
    'Theme command (⌘K 切换主题) cycles light → system → dark → light, follows OS changes only in system mode and persists after reload',
  )
  await prompt.focus()
  await page.screenshot({ path: join(output, 'dark-1440.png'), animations: 'disabled' })
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await verifyUnifiedModelPicker(page)
  await page
    .locator('.model-effort-popover')
    .screenshot({ path: join(output, 'unified-model-picker-dark.png') })
  await page.keyboard.press('Escape')
  report.checks.push(
    '320/390/768/1440 px: all three primary controls remain accessible inline or in the overflow tray without horizontal clipping; light/dark screenshots',
  )
  // 先收起检查器：下面确认没有留下文件改动的普通文本回复结束后，不会自动展开它。
  if (await page.locator('[data-session-inspector]').count())
    await page.locator('[data-inspector-toggle]').click()
  await page.locator('[data-session-inspector]').waitFor({ state: 'detached' })
  await prompt.fill('ping [pi-ui-sse]')
  await page.getByRole('button', { name: '发送消息', exact: true }).click()
  await page.getByText('流式回复：', { exact: true }).waitFor({ timeout: 60000 })
  await page.getByRole('button', { name: '停止', exact: true }).waitFor()
  report.checks.push(
    'actual composer sends prompt through Pisper HTTP/SSE',
    'partial text arrives before final response',
  )
  await page.getByText('流式回复：验收通过', { exact: true }).waitFor({ timeout: 30000 })
  // 发送键运行中也保持「发送消息」，以运行条停止键消失判断本轮结束。
  await page.getByRole('button', { name: '停止', exact: true }).waitFor({ state: 'hidden' })
  // 只有本轮留下文件改动时才自动展开“改动 · 本会话”；普通文本回复保持检查器收起。
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(await page.locator('[data-session-inspector]').count(), 0)
  report.checks.push('A completed ordinary text reply leaves the collapsed inspector closed')
  const chatRegions = await page.evaluate(() => {
    const messages = document
      .querySelector('[data-chat-region="messages"]')
      ?.getBoundingClientRect()
    const transcript = document.querySelector('.transcript')?.getBoundingClientRect()
    const composer = document
      .querySelector('[data-chat-region="composer"]')
      ?.getBoundingClientRect()
    return {
      messagesBottom: messages?.bottom,
      transcriptBottom: transcript?.bottom,
      composerTop: composer?.top,
    }
  })
  assert.ok(
    Number.isFinite(chatRegions.transcriptBottom) && Number.isFinite(chatRegions.composerTop),
  )
  assert.ok(chatRegions.transcriptBottom <= chatRegions.composerTop + 1)
  assert.ok(chatRegions.messagesBottom <= chatRegions.composerTop + 1)
  report.checks.push(
    'Transcript scroll area is bounded above the composer instead of flowing underneath it',
  )
  assert.ok(sessionId)
  const history = await api(`/api/sessions/${sessionId}/messages?limit=50`)
  assert.match(JSON.stringify(history), /流式回复：验收通过/)
  await page.screenshot({ path: join(output, 'conversation.png') })
  await page.reload()
  await page.getByText('流式回复：验收通过', { exact: true }).waitFor({ timeout: 30000 })
  report.checks.push(
    'completed assistant message persisted by release backend',
    'history survives browser reload',
  )
  await api(`/api/sessions/${sessionId}/thinking-level`, 'PUT', { level: 'high' })
  await api(`/api/sessions/${sessionId}`, 'PATCH', { name: 'PI background-run QA' })
  await page.reload()
  await page.getByText('PI background-run QA', { exact: true }).first().waitFor()
  await prompt.fill('stop-test [pi-ui-sse]')
  await page.getByRole('button', { name: '发送消息', exact: true }).click()
  await page
    .getByText(/正在生成停止测试/)
    .first()
    .waitFor({ timeout: 30000 })
  // 运行状态由侧栏“运行中”分组（计数 + 墨色弧）和看板“运行中”泳道承载。
  await fleet(1).waitFor({ timeout: 15000 })
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  assert.equal(
    await page
      .locator('.model-effort-model')
      .getByRole('combobox', { name: '当前会话模型' })
      .isEnabled(),
    true,
  )
  await verifyUnifiedModelPicker(page)
  await page
    .locator('.model-effort-popover')
    .screenshot({ path: join(output, 'unified-model-picker-running.png') })
  report.checks.push('Unified model trigger upper/lower rows remain clickable while streaming')
  assert.equal(await page.getByRole('slider', { name: '当前思考等级' }).isEnabled(), true)
  await page.getByRole('slider', { name: '当前思考等级' }).press('Home')
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
      '关闭',
  )
  assert.equal((await api(`/api/sessions/${sessionId}/thinking-level`)).thinkingLevel, 'high')
  await page.getByText('生成时可预选，本轮结束后应用。', { exact: true }).waitFor()
  // 改回当前等级可以取消预选，不向正在运行的后端发送 PUT。
  await page.getByRole('slider', { name: '当前思考等级' }).press('End')
  await page.getByRole('slider', { name: '当前思考等级' }).press('Home')
  await page.keyboard.press('Escape')
  report.checks.push(
    'reasoning remains editable during streaming; selection is staged without mutating the active run',
  )

  await page.goto(base + '/#/board/new')
  await page.waitForFunction(
    (previous) => /^#\/board\/(?!new$)[^/]+$/.test(location.hash) && location.hash !== previous,
    `#/board/${sessionId}`,
  )
  await page.getByTestId('workbench-greeting').waitFor()
  const secondaryId = decodeURIComponent(new URL(page.url()).hash.slice('#/board/'.length))
  assert.notEqual(secondaryId, sessionId)
  await api(`/api/sessions/${secondaryId}`, 'PATCH', { name: 'PI draft side-session' })
  await prompt.fill('第二会话未发送草稿')
  assert.equal(report.stopConnectionClosed, false)
  // 回到后台运行中的会话：看板“运行中”泳道的卡片打开会话页。
  await shellTab('board').click()
  const backgroundCard = page.locator(`[data-board-card="${sessionId}"]`)
  await backgroundCard.and(page.locator('[data-lane="running"]')).waitFor({ timeout: 15000 })
  await backgroundCard.click()
  await page.waitForFunction((hash) => location.hash === hash, `#/board/${sessionId}`)
  await page
    .getByText(/正在生成停止测试/)
    .first()
    .waitFor()
  assert.equal(report.stopConnectionClosed, false)
  report.checks.push(
    'switching active session unmounts the view without aborting the backend stream',
  )
  await prompt.fill('排队与撤回验收')
  await prompt.press('Enter')
  // 规格 v2 第 4.3 节：排队消息在输入框上方的托盘里，“编辑”撤回并放回输入框。
  const queuedEdit = page
    .locator('[data-composer-tray]')
    .getByRole('button', { name: '编辑排队消息', exact: true })
  await queuedEdit.waitFor({ timeout: 15000 })
  await queuedEdit.click()
  await page.waitForFunction(
    () =>
      document.querySelector('.focus-composer textarea[aria-label="任务描述"]')?.value ===
      '排队与撤回验收',
  )
  await prompt.fill('')
  report.checks.push(
    'Enter while streaming queues input into the tray; editing it restores the draft',
  )
  await page.getByRole('button', { name: '停止', exact: true }).click()
  await page
    .getByRole('button', { name: '停止', exact: true })
    .waitFor({ state: 'hidden', timeout: 30000 })
  await new Promise((done, reject) => {
    const started = Date.now()
    const timer = setInterval(() => {
      if (report.stopConnectionClosed) {
        clearInterval(timer)
        done()
      } else if (Date.now() - started > 5000) {
        clearInterval(timer)
        reject(new Error('upstream stream was not closed'))
      }
    }, 25)
  })
  assert.equal(report.stopConnectionClosed, true)
  await fleet(0).waitFor({ timeout: 15000 })
  report.checks.push(
    'The sidebar running group counts the run while it streams and clears after stop; Stop aborts backend run and closes upstream stream',
  )
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
        '关闭' && !document.querySelector('[aria-label="当前思考等级"]')?.disabled,
  )
  // 预选由与路由无关的命令队列每 750ms 探测一次运行状态后写入；轮询 API 等它落地。
  const stagedDeadline = Date.now() + 10000
  let stagedLevel
  do {
    stagedLevel = (await api(`/api/sessions/${sessionId}/thinking-level`)).thinkingLevel
    if (stagedLevel === 'off') break
    await new Promise((resolve) => setTimeout(resolve, 150))
  } while (Date.now() < stagedDeadline)
  report.stagedReasoningAppliedAfterMs = 10000 - (stagedDeadline - Date.now())
  assert.equal(stagedLevel, 'off')
  await page.keyboard.press('Escape')
  report.checks.push('staged reasoning survives session switching and is saved after cancellation')
  // 自然完成也应用模型预选；在 PUT 结束前不得启动下一轮。
  // 草稿会话没有消息，不上看板；经会话链接（同一文档内哈希跳转）切过去。
  await page.goto(`${base}/#/board/${encodeURIComponent(secondaryId)}`)
  await page.waitForFunction(
    () =>
      document.querySelector('.focus-composer textarea[aria-label="任务描述"]')?.value ===
      '第二会话未发送草稿',
  )
  report.checks.push('per-session unsent drafts survive session switching')
  await shellTab('board').click()
  await backgroundCard.click()
  await page.waitForFunction((hash) => location.hash === hash, `#/board/${sessionId}`)
  await prompt.fill('defer-model-test [pi-ui-sse]')
  await page.getByRole('button', { name: '发送消息', exact: true }).click()
  await page
    .getByText(/正在生成模型切换测试/)
    .first()
    .waitFor()
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page.locator('.model-effort-model').getByRole('combobox', { name: '当前会话模型' }).click()
  await page.getByRole('option', { name: /pi-ui-fixture-fixed/ }).click()
  await page.getByText('新模型生效后显示它支持的思考档位。', { exact: true }).waitFor()
  assert.equal(await page.getByRole('slider', { name: '当前思考等级' }).count(), 0)
  // 后端当前轮仍然使用旧模型；客户端只显示预选。
  assert.match(JSON.stringify(await api('/api/sessions')), /pi-ui-fixture-alt/)
  await page.keyboard.press('Escape')
  await page.goto(base + '/#/workflows')
  await page.waitForURL('**/#/workflows')
  await page.getByRole('heading', { name: '工作流', level: 1, exact: true }).waitFor()
  finishDeferredResponse()
  const selectionDeadline = Date.now() + 30000
  let appliedSelection
  do {
    appliedSelection = await api('/api/sessions/' + sessionId + '/thinking-level')
    if (
      appliedSelection.model === provider + '-fixed/' + model + '-fixed' &&
      appliedSelection.thinkingLevel === 'off'
    )
      break
    await new Promise((resolve) => setTimeout(resolve, 150))
  } while (Date.now() < selectionDeadline)
  assert.equal(appliedSelection.model, provider + '-fixed/' + model + '-fixed')
  assert.equal(appliedSelection.thinkingLevel, 'off')
  await fleet(0).waitFor({ timeout: 15000 })
  report.checks.push(
    'The sidebar running count clears after natural completion even while viewing workflows',
  )
  await page.goto(`${base}/#/board/${encodeURIComponent(sessionId)}`)
  await page
    .getByRole('heading', { name: '工作流', level: 1, exact: true })
    .waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: /^模型与智力 ·.*pi-ui-fixture-fixed/ }).click()
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.disabled &&
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
        '关闭',
  )
  assert.equal((await api(`/api/sessions/${sessionId}/thinking-level`)).thinkingLevel, 'off')
  assert.match(
    JSON.stringify(await api(`/api/sessions/${sessionId}/messages?limit=50`)),
    /pi-ui-fixture-fixed/,
  )
  await page.keyboard.press('Escape')
  await page.reload()
  await page.getByRole('button', { name: /^模型与智力 ·.*pi-ui-fixture-fixed/ }).waitFor()
  report.checks.push(
    'model preselection applies after natural completion while on the workflows page, reconciles supported effort, and persists on reload',
  )

  const modelSaveStarted = Promise.withResolvers()
  const modelFailure = Promise.withResolvers()
  const modelSaveHandled = Promise.withResolvers()
  const modelFailureDiagnostic = { fixture: null, interception: null, beforeRelease: null, sse: [] }
  report.deferredModelFailureDiagnostic = modelFailureDiagnostic
  const inspectChatResponse = (response) => {
    if (new URL(response.url()).pathname !== '/api/chat') return
    const entry = {
      status: response.status(),
      receivedAt: Date.now(),
      finishedAt: null,
      events: [],
    }
    modelFailureDiagnostic.sse.push(entry)
    void response
      .text()
      .then((body) => {
        entry.finishedAt = Date.now()
        entry.events = [...body.matchAll(/^event:\s*([^\r\n]+)/gm)].map((match) => match[1])
      })
      .catch((error) => {
        entry.bodyError = error.name
      })
  }
  page.on('response', inspectChatResponse)
  let modelSaveIntercepted = false
  let modelRouteError
  const failedModelPath = `/api/sessions/${sessionId}/model`
  const failedModelRoute = async (route) => {
    modelSaveIntercepted = true
    modelFailureDiagnostic.interception = { at: Date.now(), fixture: { ...activeDeferredFixture } }
    modelSaveStarted.resolve()
    try {
      // 释放动作由下方 finally 保证；路由回调不向事件循环抛出未处理拒绝，
      // 否则浏览器断言还没输出就会直接退出，丢失失败截图和报告。
      await modelFailure.promise
      await route.fulfill({ status: 503, json: { error: 'UI fixture rejected model save' } })
    } catch (error) {
      modelRouteError = error
    } finally {
      modelSaveHandled.resolve()
    }
  }
  await page.route(`**${failedModelPath}`, failedModelRoute)
  report.expectedFailedApi.push({ path: failedModelPath, status: 503 })
  try {
    await prompt.fill('defer-model-test failure [pi-ui-sse]')
    await page.getByRole('button', { name: '发送消息', exact: true }).click()
    await page.getByRole('button', { name: '停止', exact: true }).waitFor()
    // Wait for this fixture request, not text left by the previous turn.
    await page
      .getByText(/正在生成模型切换测试/)
      .nth(1)
      .waitFor()
    await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
    await page
      .locator('.model-effort-model')
      .getByRole('combobox', { name: '当前会话模型' })
      .click()
    await page.getByRole('option', { name: /pi-ui-fixture-alt/ }).click()
    // 先等待内层 Select 关闭，避免 Escape 被它消费后外层模型面板仍遮挡错误详情。
    await page.getByRole('listbox').waitFor({ state: 'hidden' })
    await page.keyboard.press('Escape')
    await page.locator('.model-effort-popover').waitFor({ state: 'hidden' })
    finishDeferredResponse()
    modelFailureDiagnostic.fixture = { ...activeDeferredFixture }
    await within(modelSaveStarted.promise, 'deferred model save starts')
    // 模型切换写入期间，发送键的可访问名称带上等待原因并以 aria-disabled 阻止发送
    // （982c4705 起的行为；基线在这里按旧名称“发送消息”等待而超时）。
    const blockedSend = page.getByRole('button', {
      name: '发送消息: 正在切换模型，请稍候',
      exact: true,
    })
    await blockedSend.waitFor({ timeout: 10000 })
    await prompt.fill('保存失败仍然保留草稿', { timeout: 10000 })
    assert.equal(await blockedSend.isDisabled(), true)
    const requestCount = report.requests
    await prompt.press('Enter', { timeout: 10000 })
    assert.equal(await prompt.inputValue(), '保存失败仍然保留草稿')
    assert.equal(report.requests, requestCount)
    modelFailure.resolve()
    await page.waitForFunction(() => !document.querySelector('[aria-label="发送消息"]')?.disabled)
    await page.getByRole('button', { name: '查看详情', exact: true }).last().click()
    await page.getByText('UI fixture rejected model save', { exact: true }).waitFor()
    await page.getByRole('button', { name: /^模型与智力 ·.*pi-ui-fixture-fixed/ }).waitFor()
    await page.waitForFunction(() => !document.querySelector('[aria-label="发送消息"]')?.disabled)
    assert.equal(await prompt.inputValue(), '保存失败仍然保留草稿')
    assert.equal(
      (await api(`/api/sessions/${sessionId}/thinking-level`)).model,
      `${provider}-fixed/${model}-fixed`,
    )
  } catch (error) {
    const snapshot = await within(
      Promise.all([api(`/api/sessions/${sessionId}/live`), api('/api/sessions')]),
      'deferred model failure diagnostics',
      5000,
    )
    const [live, catalog] = snapshot
    const fields = (value) =>
      value &&
      Object.fromEntries(
        [
          'id',
          'streaming',
          'model',
          'startedAt',
          'finishedAt',
          'lastActivityAt',
          'lifecycle',
          'error',
        ].map((key) => [key, value[key]]),
      )
    modelFailureDiagnostic.beforeRelease = {
      at: Date.now(),
      fixture: { ...activeDeferredFixture },
      live: fields(live),
      catalog: fields(catalog.sessions.find((session) => session.id === sessionId)),
    }
    throw error
  } finally {
    modelFailure.resolve()
    if (modelSaveIntercepted)
      await within(modelSaveHandled.promise, 'injected model failure settles')
    await page.unroute(`**${failedModelPath}`, failedModelRoute)
    page.off('response', inspectChatResponse)
  }
  if (modelRouteError) throw modelRouteError
  await prompt.fill('')
  report.checks.push(
    'deferred save blocks both send paths; failure restores actual model without losing draft or retrying the write',
  )

  const sideChatParentId = sessionId
  await verifySideChat({ page, base, api, report, output, parentSessionId: sideChatParentId })
  sessionId = sideChatParentId

  // 使用真实历史页操作，避免从测试进程直接删除仍被活动视图读取的会话。
  await page.goto(base + '/#/chat/history')
  await page.getByRole('heading', { level: 1, name: '历史会话', exact: true }).waitFor()
  await page.getByRole('button', { name: 'PI background-run QA 的更多操作', exact: true }).click()
  await page.getByRole('menuitem', { name: '重命名会话', exact: true }).click()
  const renameDialog = page.getByRole('dialog', { name: '重命名会话', exact: true })
  await renameDialog.getByRole('textbox', { name: '会话标题', exact: true }).fill('PI UI CRUD 验收')
  await renameDialog.getByRole('button', { name: '保存', exact: true }).click()
  await renameDialog.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: 'PI UI CRUD 验收 的更多操作', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: 'PI UI CRUD 验收 的更多操作', exact: true }).waitFor()
  report.checks.push('session rename through history UI persists across reload')
  for (const [id, name] of [
    [secondaryId, 'PI draft side-session'],
    [sessionId, 'PI UI CRUD 验收'],
  ]) {
    const actions = page.getByRole('button', { name: `${name} 的更多操作`, exact: true })
    await actions.click()
    await page.getByRole('menuitem', { name: '删除会话', exact: true }).click()
    const confirmation = page.getByRole('dialog', { name: '删除会话', exact: true })
    const [deleted] = await Promise.all([
      page.waitForResponse(
        (r) =>
          new URL(r.url()).pathname === `/api/sessions/${id}` && r.request().method() === 'DELETE',
      ),
      confirmation.getByRole('button', { name: '删除', exact: true }).click(),
    ])
    assert.ok(deleted.ok())
    await confirmation.waitFor({ state: 'hidden' })
    await actions.waitFor({ state: 'hidden' })
  }
  const sessions = await api('/api/sessions')
  assert.ok(!JSON.stringify(sessions).includes(sessionId))
  assert.ok(!JSON.stringify(sessions).includes(secondaryId))
  report.checks.push('session deletion through confirmed history UI and release API')
  await verifyPisperIteration({ page, base, api, report, output, provider, alternate })

  await verifySessionTreeLifecycle({ page, base, api, report, output })

  await verifyMemoryLifecycle({ page, base, api, runtime: runtime.runtime, report, output })

  const routes = [
    ['/chat/history', 'chatHistory', '历史会话'],
    ['/assets', 'assets', '资产'],
    ['/workflows', 'workflows', '工作流'],
    ['/workflows/new', 'workflowCreate', '新建工作流'],
    ['/schedules', 'schedules', '定时任务'],
    ['/plugins', 'plugins', '插件'],
    ['/mcp', 'mcp', 'MCP'],
    ['/skills', 'skills', '技能'],
    ['/memory', 'memory', '星忆'],
    ['/channels', 'channels', '渠道'],
    ['/components', 'components', '组件工作室'],
    ['/more', 'more', '功能'],
    ...[
      // 设置分区的 h1 是短名（A6）；远程访问 / 桌面宠物是功能页，仍是完整名。
      ['models', 'models-connections', '模型'],
      ['notifications', 'notifications-browser', '通知'],
      ['interface', 'interface-appearance', '界面'],
      ['shortcuts', 'shortcuts-bindings', '快捷键'],
      ['desktop-pet', 'desktop-pet-settings', '桌面宠物'],
      ['updates', 'updates-main', '更新'],
      ['remote-access', 'remote-access-main', '远程访问'],
      ['about', 'about-project', '关于'],
    ].map(([section, anchor, heading]) => ['/config/' + section, 'config', heading, anchor]),
  ]
  report.routes = []
  for (const [route, pageId, heading, anchor] of routes) {
    await page.goto(base + '/#' + route)
    await page
      .getByRole('heading', { level: 1, name: heading, exact: true })
      .waitFor({ timeout: 45000 })
    // Hash 导航不会等待 React 换页；核对目标根与设置卡片，不能误将旧页面判为通过。
    const selector = `.page-content.page-${pageId}`
    await page.locator(selector).waitFor()
    if (anchor) await page.locator(`${selector} [data-config-card="${anchor}"]`).waitFor()
    await page.waitForFunction((selector) => {
      const content = document.querySelector(selector)
      return content && content.innerText.length > 35 && !/^加载中/.test(content.innerText)
    }, selector)
    const text = await page.locator('main').innerText()
    assert.ok(
      !/Unexpected Application Error|Application error|Cannot read properties|Minified React error/.test(
        text,
      ),
      route,
    )
    assert.equal(await page.locator('main h1').first().innerText(), heading)
    report.routes.push({ route, heading, resolvedHash: new URL(page.url()).hash })
  }
  report.checks.push('all 21 release feature/configuration routes render without a React error')
  await verifyPageTemplates({ page, base, api, workspace, output, report })
  // 新外壳的布局切换：桌面（含窄到 651px 的窗口）由侧栏承载一级导航（任何宽度默认展开），
  // 手机宽度换成手机外壳；切换与刷新都不丢会话草稿，设置入口点亮“设置”。
  await page.goto(base + '/#/board/new')
  await page.waitForFunction(() => /^#\/board\/(?!new$)[^/]+$/.test(location.hash))
  const layoutHash = new URL(page.url()).hash
  await prompt.waitFor()
  await prompt.fill('layout and titlebar draft')
  const desktopViewport = page.viewportSize()
  for (const width of [1080, 800, 651]) {
    await page.setViewportSize({ width, height: 700 })
    await titlebar.waitFor()
    assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
    assert.equal(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
      true,
      `${width}px sidebar shell must not overflow the window`,
    )
    for (const tab of ['board', 'more', 'settings']) {
      const bounds = await shellTab(tab).boundingBox()
      assert.ok(
        bounds && bounds.height >= 28 && bounds.x >= 0 && bounds.x + bounds.width <= width,
        `${tab} entry stays reachable at ${width}px: ${JSON.stringify(bounds)}`,
      )
    }
  }
  await page.setViewportSize({ width: 650, height: 700 })
  await page.locator('[data-session-header="phone"]').waitFor()
  assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
  assert.equal(await titlebar.count(), 0)
  assert.equal(
    await page.evaluate(() => document.documentElement.hasAttribute('data-phone-shell')),
    true,
  )
  assert.equal(await prompt.inputValue(), 'layout and titlebar draft')
  await page.setViewportSize(desktopViewport)
  await titlebar.waitFor()
  assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
  await shellTab('settings').click()
  await page.waitForURL('**/#/config/models')
  await page
    .locator(
      '[data-shell-sidebar="settings"] [data-settings-section="models"][aria-current="page"]',
    )
    .waitFor()
  await page.goto(base + '/' + layoutHash)
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'layout and titlebar draft')
  // 会话页不点亮任何入口（当前会话由侧栏的会话行标出）。
  await page.waitForFunction(
    () => !document.querySelector('[data-shell-sidebar] [data-sidebar-entry][aria-current]'),
  )
  await page.reload()
  await titlebar.waitFor()
  await prompt.waitFor()
  // 草稿按既有契约只在内存保留；刷新后重建草稿，验证后续布局切换。
  await prompt.fill('layout and titlebar draft')
  await page.setViewportSize({ width: 390, height: 844 })
  const preservedContextSheet = page.locator('[data-slot="sheet-content"]')
  if (await preservedContextSheet.isVisible()) {
    await page.keyboard.press('Escape')
    await preservedContextSheet.waitFor({ state: 'hidden' })
  }
  await page.locator('[data-session-header="phone"]').waitFor()
  assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
  assert.equal(await titlebar.count(), 0)
  await page.setViewportSize(desktopViewport)
  await titlebar.waitFor()
  assert.equal(await prompt.inputValue(), 'layout and titlebar draft')
  report.checks.push(
    'The expanded sidebar keeps its entries reachable without overflow down to 651px and phones switch to the phone shell; the session draft survives layout switching, settings navigation and the round trip back',
  )
  // 独立组件已迁到组件页：旧的 ?view=widgets 链接直达组件页（侧栏点亮“功能”），聊天布局切换器不再出现。
  await page.goto(base + '/#/config/interface?view=widgets')
  await page.waitForURL('**/#/components')
  await page.locator('.page-content.page-components').waitFor()
  await titlebar.locator('[data-sidebar-entry="features"][aria-current="page"]').waitFor()
  assert.equal(await page.getByRole('tab', { name: '会话布局' }).count(), 0)
  // 旧的 /chat 入口回到看板；会话草稿仍在原会话里。
  await page.goto(base + '/#/chat')
  // 主页改为会话：/chat 回到主页（上次停留的会话或新任务草稿），不再回看板。
  await page.waitForURL((url) => /^#\/board\/[^/]+$/.test(url.hash) || url.hash === '#/')
  await page.locator('[data-session-shell]').waitFor()
  await page.goto(base + '/' + layoutHash)
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'layout and titlebar draft')
  assert.equal(await page.getByRole('button', { name: '会话布局' }).count(), 0)
  report.checks.push(
    'Old widget-settings links open the components page (the sidebar lights 功能), /chat returns to the home (last session or new-task draft), the removed chat layout switcher stays absent and navigation preserves the chat draft',
  )

  await verifyRemoteWorkspaceSettings({ browser, base, report, output })
  await verifyFirstRunSetup({
    browser,
    root,
    output,
    modelBaseUrl: `http://127.0.0.1:${fixture.address().port}/${provider}/v1`,
    modelId: model,
    report,
  })

  assert.deepEqual(report.pageErrors, [])
  assert.deepEqual(report.failedApi, report.expectedFailedApi)
  report.status = 'passed'
  console.log(JSON.stringify(report, null, 2))
} catch (error) {
  report.status = 'failed'
  report.failure = String(error)
  if (page && !page.isClosed()) {
    await page.screenshot({ path: join(output, 'failure.png') })
    console.log((await page.locator('body').ariaSnapshot()).slice(-8000))
  }
  throw error
} finally {
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  fixture.closeAllConnections()
  await new Promise((resolve) => fixture.close(resolve))
  await runtime.close()
  for (const directory of [dataDir, workspace]) {
    assert.equal(dirname(directory), output, 'Unsafe temporary cleanup path')
    await rm(directory, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 })
  }
  console.log(`UI smoke report and screenshots: ${output}`)
}
