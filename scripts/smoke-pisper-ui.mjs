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
// 思考等级和模型名称共用一个选择器，上下两行、键盘和触发器间留白都可操作。
async function verifyUnifiedModelPicker(page) {
  const panel = page.locator('.model-effort-popover')
  const picker = panel.getByRole('combobox', { name: '当前会话模型' })
  assert.equal(await panel.getByRole('combobox').count(), 1)
  assert.equal(await picker.isEnabled(), true)
  const bounds = await picker.boundingBox()
  assert.ok(bounds && bounds.height >= 48 && bounds.height <= 60)
  for (const fraction of [0.25, 0.5, 0.75]) {
    await picker.click({ position: { x: bounds.width / 2, y: bounds.height * fraction } })
    await page.getByRole('listbox').waitFor()
    await page.keyboard.press('Escape')
    await page.getByRole('listbox').waitFor({ state: 'hidden' })
    assert.equal(await picker.evaluate((el) => el === document.activeElement), true)
  }
  await picker.press('Enter')
  await page.getByRole('listbox').waitFor()
  await page.keyboard.press('Escape')
  await page.getByRole('listbox').waitFor({ state: 'hidden' })
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
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
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
  await page.goto(base + '/#/chat')
  await page.getByRole('textbox', { name: '任务描述' }).waitFor({ timeout: 60000 })
  await page.getByTestId('workbench-new-task').click()
  await page.getByTestId('workbench-greeting').waitFor()
  const prompt = page.getByRole('textbox', { name: '任务描述' })
  await page.locator('[data-brand="Pisper"]').waitFor()
  const suggestions = page.getByRole('region', { name: '试试这些任务', exact: true })
  await suggestions.getByRole('button', { name: '解释代码', exact: true }).click()
  assert.ok((await prompt.inputValue()).length > 0)
  assert.equal(report.requests, 0)
  assert.ok((await suggestions.boundingBox()).y > (await prompt.boundingBox()).y)
  await prompt.fill('')
  report.checks.push(
    'New conversation has P watermark and task suggestions below the composer; clicking a suggestion fills but never sends',
  )
  const nav = page.getByTestId('workbench-sidebar')
  for (const label of ['工作流', '资产'])
    assert.equal(await nav.getByRole('button', { name: label, exact: true }).count(), 1)
  for (const label of ['自动化', '插件', '终端'])
    assert.equal(await nav.getByRole('button', { name: label, exact: true }).count(), 0)
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
  await nav.getByRole('button', { name: '设置', exact: true }).click()
  await page.locator('[data-model-provider-split-panel]').waitFor()
  await page.getByRole('heading', { name: '设置', level: 1, exact: true }).waitFor()
  assert.match(page.url(), /config\/models/)
  const connectionList = page
    .locator('[data-model-provider-split-panel]')
    .getByRole('navigation', { name: '连接', exact: true })
  const visibleProviders = (await api('/api/config')).providers.filter(
    (item) => item.configured || item.custom,
  )
  assert.deepEqual(
    await connectionList
      .getByRole('button')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute('aria-label'))),
    visibleProviders.map((item) => item.name),
  )
  const defaultConnection = connectionList.getByRole('button', {
    name: 'PI Local UI Test',
    exact: true,
  })
  await defaultConnection.getByText('默认', { exact: true }).waitFor()
  await connectionList.getByRole('button', { name: 'PI Alternate UI Test', exact: true }).click()
  assert.equal(await defaultConnection.getByText('默认', { exact: true }).isVisible(), true)
  assert.equal(await defaultConnection.getAttribute('aria-current'), null)
  assert.equal(await connectionList.getByText('默认', { exact: true }).count(), 1)
  await defaultConnection.click()
  report.checks.push(
    'Default provider badge remains on the default connection when viewing another connection',
  )
  const quickSetup = page.locator('main > header').getByRole('button', {
    name: '快速设置',
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
  assert.equal(formFont, '14px')
  await page.screenshot({ path: join(output, 'model-settings-refined.png') })
  await within(catalogReady.promise, 'catalog refresh captures old configuration')
  const configForm = page.locator('[data-provider-connection-editor]')
  const nameInput = configForm.getByLabel('显示名称', { exact: true })
  const originalProviderName = await nameInput.inputValue()
  const savedProviderName = 'PI UI Saved During Catalog Refresh'
  await nameInput.fill(savedProviderName)
  await configForm.getByRole('button', { name: '保存修改', exact: true }).click()
  await page.getByRole('heading', { name: savedProviderName, exact: true }).waitFor()
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
    await page.locator('[data-model-provider-split-panel] h2').textContent(),
    savedProviderName,
    'late catalog must preserve the saved provider heading',
  )
  await page.unroute('**/api/providers/models/refresh')
  await nameInput.fill(originalProviderName)
  await configForm.getByRole('button', { name: '保存修改', exact: true }).click()
  await page.getByRole('heading', { name: originalProviderName, exact: true }).waitFor()
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
      await page
        .locator('[data-model-provider-split-panel]')
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
    await page.getByRole('heading', { name: connectionName, exact: true }).waitFor()
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
        .getAttribute('aria-current'),
      'true',
      'a committed connection stays selected after retry or close',
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
    await page.getByRole('heading', { name: connectionName, exact: true }).waitFor()
    await page.getByRole('button', { name: originalProviderName, exact: true }).click()
    await api(`/api/providers/${connectionId}`, 'DELETE')
  }
  report.checks.push(
    'Quick setup discovers models or accepts manual IDs after discovery failure; partial creation stays visible and selected, retry creates only once, and close retains the saved connection',
    'Model deletion supports cancel and confirmation, removes only the selected model, and retains an empty provider connection after the last model is deleted',
  )

  await nav
    .getByRole('navigation', { name: '设置导航', exact: true })
    .getByRole('button', { name: '界面设置', exact: true })
    .click()
  await page.waitForURL('**/#/config/interface')
  await nav.getByRole('button', { name: '主页', exact: true }).click()
  await page.waitForURL('**/#/chat')
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'return home keeps this draft')
  await prompt.fill('')
  report.checks.push(
    '32px send button; workflows/assets primary navigation; Settings opens complete settings navigation and Home restores the conversation draft',
    'Provider navigation shows configured or custom connections with one Quick setup entry and no preset catalog',
  )

  assert.deepEqual(
    await page
      .locator('.focus-composer-visible-tools [data-composer-tool-id]')
      .evaluateAll((nodes) => nodes.map((node) => node.dataset.composerToolId)),
    ['permission', 'run-mode', 'model'],
  )
  assert.equal(
    await page.getByRole('button', { name: '审批模式：完全访问', exact: true }).count(),
    0,
  )
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page
    .locator('.model-effort-model')
    .getByRole('combobox', { name: '当前会话模型' })
    .waitFor()
  await page.waitForFunction(
    () =>
      Math.abs(
        document.querySelector('.model-effort-popover').getBoundingClientRect().width - 224,
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
  await page.getByRole('button', { name: /^执行模式 · Plan 模式/ }).click()
  await page.getByRole('menuitemradio', { name: /Goal 模式/ }).click()
  await page.getByRole('button', { name: /^执行模式 · Goal 模式/ }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: /^执行模式 · Goal 模式/ }).click()
  await page.getByRole('menuitemradio', { name: /Plan 模式/ }).click()
  await page.getByRole('button', { name: /^执行模式 · Plan 模式/ }).waitFor()
  report.checks.push(
    'visible Plan/Goal selector changes backend mode, persists on reload, restores Plan',
  )
  await page.getByRole('button', { name: /^审批模式：/ }).click()
  await page.getByRole('menuitemradio', { name: /^自动审批/ }).click()
  await page.getByRole('button', { name: '审批模式：自动审批', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: '审批模式：自动审批', exact: true }).waitFor()
  await page.getByRole('button', { name: '审批模式：自动审批', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^审批后写入/ }).click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).waitFor()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^完全访问/ }).click()
  const accessConfirmation = page.getByRole('dialog', { name: '启用完全访问', exact: true })
  await accessConfirmation.getByRole('button', { name: '取消', exact: true }).click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^完全访问/ }).click()
  await accessConfirmation.getByRole('button', { name: '启用完全访问', exact: true }).click()
  await page.getByRole('button', { name: '审批模式：完全访问', exact: true }).waitFor()
  await page.reload()
  await page.getByRole('button', { name: '审批模式：完全访问', exact: true }).click()
  await page.getByRole('menuitemradio', { name: /^审批后写入/ }).click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).waitFor()
  report.checks.push(
    'approval/auto/full access are real persisted settings; no elevated default; restored approval-required',
  )
  assert.equal(await page.getByRole('button', { name: '打开会话操作菜单', exact: true }).count(), 0)
  assert.equal(await page.getByRole('button', { name: /拆分到|关闭标签/ }).count(), 0)
  report.checks.push('no split-window or uncloseable pane entries in session menu')
  await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
  await page.getByRole('button', { name: '自定义快捷方式', exact: true }).click()
  await page.getByRole('button', { name: '模型与智力：移入收纳区', exact: true }).click()
  await page.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.reload()
  await prompt.waitFor()
  assert.equal(
    await page.locator('.focus-composer-visible-tools [data-composer-tool-id="model"]').count(),
    0,
  )
  await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
  await page
    .getByRole('toolbar', { name: '快捷操作' })
    .getByRole('button', { name: /^模型与智力 ·/ })
    .click()
  await page.getByRole('slider', { name: '当前思考等级' }).waitFor()
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: '自定义快捷方式', exact: true }).click()
  await page.getByRole('button', { name: '恢复默认', exact: true }).click()
  await page.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.keyboard.press('Escape')
  await page.locator('.focus-composer-visible-tools [data-composer-tool-id="model"]').waitFor()
  report.checks.push(
    'toolbar customize/move to overflow persists; restore defaults returns all three inline controls',
  )
  await page.getByRole('button', { name: /^(展开|收起)快捷操作$/ }).waitFor()
  if ((await page.locator('.composer-tools-trigger').getAttribute('aria-expanded')) === 'true')
    await page.locator('.composer-tools-trigger').click()
  await page.getByRole('button', { name: '展开快捷操作', exact: true }).waitFor()
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

  await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
  const tools = await page
    .locator('[data-composer-tool-id]')
    .evaluateAll((nodes) => nodes.map((n) => n.dataset.composerToolId))
  assert.ok(
    tools.includes('attachment') &&
      tools.includes('resource') &&
      tools.includes('run-mode') &&
      tools.includes('commands') &&
      tools.includes('compact-context'),
  )
  report.checks.push('plus tray exposes attachments/resources/visual/run-mode/commands/compaction')
  assert.equal(
    await page.locator('.composer-tool-tray').evaluate((el) => getComputedStyle(el).flexDirection),
    'column',
  )
  assert.equal(
    await page
      .getByRole('toolbar', { name: '快捷操作' })
      .getByRole('region', { name: '试试这些任务' })
      .count(),
    0,
  )
  console.log('TOOLS', tools)
  await page.keyboard.press('Escape')
  await page.getByRole('button', { name: '打开会话上下文', exact: true }).click()
  await page.getByRole('tab', { name: '文件改动', exact: true }).waitFor()
  for (const name of ['计划', '网页预览']) {
    await page.getByRole('button', { name: '新增辅助页面', exact: true }).click()
    await page.getByRole('menuitem', { name, exact: true }).click()
    await page.getByRole('menu', { name: '新增辅助页面', exact: true }).waitFor({ state: 'hidden' })
    await page.getByRole('tab', { name, exact: true }).waitFor()
  }
  for (const name of ['文件改动', '计划', '网页预览'])
    assert.equal(await page.getByRole('tab', { name, exact: true }).count(), 1)
  await page.getByRole('tab', { name: '文件改动', exact: true }).focus()
  await page.keyboard.press('End')
  assert.equal(
    await page.getByRole('tab', { name: '网页预览', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await page.keyboard.press('Home')
  await page.keyboard.press('ArrowRight')
  assert.equal(
    await page.getByRole('tab', { name: '计划', exact: true }).getAttribute('aria-selected'),
    'true',
  )
  await page.getByRole('button', { name: '关闭会话上下文', exact: true }).last().click()
  await page.getByRole('button', { name: '审批模式：审批后写入', exact: true }).click()
  console.log('PERMISSIONS', await page.getByRole('menu').ariaSnapshot())
  await page.keyboard.press('Escape')
  report.checks.push('right context files/plan/web-preview keyboard tabs and close/reopen')
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 })
    await page.waitForFunction((expected) => window.innerWidth === expected, width)
    if (width === 390) {
      // 浏览器没有系统栏；模拟 Android 原生注入的真实 inset，检查最终位置而非 CSS 写法。
      await page.evaluate(() => {
        document.documentElement.style.setProperty('--pisper-safe-area-top', '24px')
        document.documentElement.style.setProperty('--pisper-safe-area-bottom', '16px')
      })
      await page.getByRole('button', { name: '打开会话上下文', exact: true }).click()
      const sheet = page.locator('[data-slot="sheet-content"]')
      await sheet.waitFor()
      const bounds = await sheet.evaluate((element) => {
        const rect = element.getBoundingClientRect()
        return { top: rect.top, bottom: rect.bottom, viewportHeight: innerHeight }
      })
      assert.equal(bounds.top, 24)
      assert.equal(bounds.bottom, bounds.viewportHeight - 16)
      await sheet
        .getByRole('button', { name: /关闭辅助页面/ })
        .first()
        .click()
      await page.keyboard.press('Escape')
      await sheet.waitFor({ state: 'hidden' })
      await page.evaluate(() => {
        document.documentElement.style.removeProperty('--pisper-safe-area-top')
        document.documentElement.style.removeProperty('--pisper-safe-area-bottom')
      })
      report.checks.push(
        '390px context sheet respects system safe areas and its tab close remains operable',
      )
    }
    const inline = page.locator('.focus-composer-visible-tools')
    for (const id of ['permission', 'run-mode', 'model']) {
      if (await inline.locator(`[data-composer-tool-id="${id}"]`).isVisible()) continue
      await page.getByRole('button', { name: '展开快捷操作', exact: true }).click()
      await page.waitForFunction(
        (id) =>
          [...document.querySelectorAll('[data-composer-tool-id="' + id + '"]')].some((node) =>
            node.checkVisibility(),
          ),
        id,
      )
      assert.ok(
        (await inline.locator(`[data-composer-tool-id="${id}"]`).isVisible()) ||
          (await page
            .getByRole('toolbar', { name: '快捷操作' })
            .locator(`[data-composer-tool-id="${id}"]`)
            .isVisible()),
        `${id} accessible inline or in overflow at ${width}`,
      )
      await page.keyboard.press('Escape')
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
    await page.screenshot({ path: join(output, `light-${width}.png`), animations: 'disabled' })
  }
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  const themeToggle = page.getByTestId('theme-toggle')
  await page.emulateMedia({ colorScheme: 'light' })
  assert.equal(await themeToggle.getAttribute('aria-label'), '主题：浅色，点击切换为跟随系统')
  await themeToggle.click()
  assert.equal(await themeToggle.getAttribute('aria-label'), '主题：跟随系统，点击切换为深色')
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  await page.emulateMedia({ colorScheme: 'dark' })
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await page.emulateMedia({ colorScheme: 'light' })
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  await themeToggle.click()
  assert.equal(await themeToggle.getAttribute('aria-label'), '主题：深色，点击切换为浅色')
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await themeToggle.click()
  assert.equal(await themeToggle.getAttribute('aria-label'), '主题：浅色，点击切换为跟随系统')
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light')
  await themeToggle.click()
  await themeToggle.click()
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await page.reload()
  await prompt.waitFor()
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'dark')
  await themeToggle.waitFor()
  assert.equal(await themeToggle.getAttribute('aria-label'), '主题：深色，点击切换为浅色')
  report.checks.push(
    'Theme toggle cycles system/dark/light with current and next labels, follows OS changes only in system mode and persists after reload',
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
  await prompt.fill('ping [pi-ui-sse]')
  await page.getByRole('button', { name: '发送消息', exact: true }).click()
  await page.getByText('流式回复：', { exact: true }).waitFor({ timeout: 60000 })
  await page.getByRole('button', { name: '停止', exact: true }).waitFor()
  report.checks.push(
    'actual composer sends prompt through Pisper HTTP/SSE',
    'partial text arrives before final response',
  )
  await page.getByText('流式回复：验收通过', { exact: true }).waitFor({ timeout: 30000 })
  await page.getByRole('button', { name: '发送消息', exact: true }).waitFor()
  await page.getByRole('button', { name: '打开会话上下文', exact: true }).waitFor()
  assert.equal(await page.getByRole('button', { name: '关闭会话上下文', exact: true }).count(), 0)
  report.checks.push('A completed ordinary text reply leaves the empty context panel closed')
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
  await page
    .getByRole('button', { name: /^正在执行 PI background-run QA / })
    .locator('svg.animate-spin')
    .waitFor()
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

  await page.getByTestId('workbench-new-task').click()
  await page.getByTestId('workbench-greeting').waitFor()
  const secondaryId = await page.evaluate(() => localStorage.getItem('pisper-active-session'))
  assert.notEqual(secondaryId, sessionId)
  await api(`/api/sessions/${secondaryId}`, 'PATCH', { name: 'PI draft side-session' })
  await prompt.fill('第二会话未发送草稿')
  assert.equal(report.stopConnectionClosed, false)
  await page.getByRole('button', { name: /^(正在执行 )?PI background-run QA / }).click()
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
  await page.getByRole('button', { name: '撤回消息', exact: true }).waitFor({ timeout: 15000 })
  await page.getByRole('button', { name: '撤回消息', exact: true }).click()
  await page.waitForFunction(
    () => document.querySelector('textarea[aria-label="任务描述"]')?.value === '排队与撤回验收',
  )
  await prompt.fill('')
  report.checks.push('Enter while streaming queues input; withdrawing restores the draft')
  await page.getByRole('button', { name: '停止', exact: true }).click()
  await page.getByRole('button', { name: '发送消息', exact: true }).waitFor({ timeout: 30000 })
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
  await page.getByRole('button', { name: /^(未读 )?PI background-run QA / }).waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: /^(未读 )?PI background-run QA / })
      .locator('svg.animate-spin')
      .count(),
    0,
  )
  report.checks.push(
    'Session spinner appears while running and clears after stop; Stop aborts backend run and closes upstream stream',
  )
  await page.getByRole('button', { name: /^模型与智力 ·/ }).click()
  await page.waitForFunction(
    () =>
      document.querySelector('[aria-label="当前思考等级"]')?.getAttribute('aria-valuetext') ===
        '关闭' && !document.querySelector('[aria-label="当前思考等级"]')?.disabled,
  )
  assert.equal((await api(`/api/sessions/${sessionId}/thinking-level`)).thinkingLevel, 'off')
  await page.keyboard.press('Escape')
  report.checks.push('staged reasoning survives session switching and is saved after cancellation')
  // 自然完成也应用模型预选；在 PUT 结束前不得启动下一轮。
  await page.getByRole('button', { name: /^PI draft side-session / }).click()
  await page.waitForFunction(
    () => document.querySelector('textarea[aria-label="任务描述"]')?.value === '第二会话未发送草稿',
  )
  report.checks.push('per-session unsent drafts survive session switching')
  await page.getByRole('button', { name: /^(正在执行 )?PI background-run QA / }).click()
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
  await page.getByRole('button', { name: /^(未读 )?PI background-run QA / }).waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: /^(未读 )?PI background-run QA / })
      .locator('svg.animate-spin')
      .count(),
    0,
  )
  report.checks.push('Session spinner clears after natural completion even while viewing workflows')
  await page.goto(base + '/#/chat')
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
    await page.getByRole('button', { name: '发送消息', exact: true }).waitFor({ timeout: 10000 })
    await prompt.fill('保存失败仍然保留草稿', { timeout: 10000 })
    assert.equal(
      await page.getByRole('button', { name: '发送消息', exact: true }).isDisabled(),
      true,
    )
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
    ['/decisions', 'decisions', '决策模型'],
    ['/memory', 'memory', '星忆'],
    ['/channels', 'channels', '渠道'],
    ['/components', 'config', '设置', 'interface-custom-ui'],
    ...[
      ['models', 'models-connections'],
      ['notifications', 'notifications-browser'],
      ['interface', 'interface-appearance'],
      ['shortcuts', 'shortcuts-bindings'],
      ['desktop-pet', 'desktop-pet-settings'],
      ['updates', 'updates-main'],
      ['remote-access', 'remote-access-main'],
      ['about', 'about-project'],
    ].map(([section, anchor]) => ['/config/' + section, 'config', '设置', anchor]),
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
  report.checks.push('all 20 release feature/configuration routes render without a React error')
  await page.goto(base + '/#/chat')
  await prompt.waitFor()
  await prompt.fill('layout and sidebar draft')
  const sidebar = page.locator('[data-slot="sidebar"][data-state]')
  assert.equal(await sidebar.getAttribute('data-state'), 'expanded')
  const collapseSidebar = page.getByRole('button', { name: '收起侧栏', exact: true })
  const expandSidebar = page.getByRole('button', { name: '展开侧栏', exact: true })
  assert.equal(await collapseSidebar.count(), 1)
  await collapseSidebar.press('Enter')
  await page.waitForFunction(
    () =>
      document.querySelector('[data-slot="sidebar"][data-state]')?.getAttribute('data-state') ===
        'collapsed' && document.activeElement?.id === 'workbench-sidebar-expand',
  )
  assert.equal(await expandSidebar.getAttribute('aria-expanded'), 'false')
  const desktopViewport = page.viewportSize()
  await page.waitForFunction(
    () => document.querySelector('[data-slot="sidebar-gap"]')?.getBoundingClientRect().width === 64,
  )
  for (const name of ['主页', '新任务', '搜索', '工作流', '资产', '更多工具', '设置']) {
    const button = nav.getByRole('button', { name, exact: true })
    assert.equal(await button.isVisible(), true)
    assert.equal(await button.getAttribute('title'), name)
  }
  await page.setViewportSize({ width: desktopViewport.width, height: 400 })
  assert.ok(
    await expandSidebar.evaluate((el) => {
      const bounds = el.getBoundingClientRect()
      return bounds.top >= 0 && bounds.bottom <= window.innerHeight
    }),
  )
  await page.setViewportSize(desktopViewport)
  for (const width of [800, 651]) {
    await page.setViewportSize({ width, height: 700 })
    await page.waitForFunction(
      () =>
        document.querySelector('[data-slot="sidebar-gap"]')?.getBoundingClientRect().width === 64,
    )
    assert.equal(await sidebar.isVisible(), true, `${width}px must keep the desktop sidebar`)
    assert.equal(await page.locator('[data-slot="sidebar"][data-mobile="true"]').count(), 0)
    for (const name of ['主页', '新任务', '搜索', '工作流', '资产', '设置']) {
      const icon = nav.getByRole('button', { name, exact: true }).locator('svg').first()
      const bounds = await icon.boundingBox()
      assert.ok(bounds && bounds.width > 0 && bounds.height > 0, `${width}px ${name} icon`)
    }
  }
  await page.setViewportSize({ width: 650, height: 700 })
  await sidebar.waitFor({ state: 'hidden' })
  await expandSidebar.waitFor()
  await page.setViewportSize(desktopViewport)
  await sidebar.waitFor()
  assert.equal(await nav.getByRole('region', { name: '最近会话', exact: true }).isVisible(), false)
  await nav.getByRole('button', { name: '更多工具', exact: true }).click()
  await page.getByRole('menu').waitFor()
  await page.keyboard.press('Escape')
  await nav.getByRole('button', { name: '设置', exact: true }).click()
  await page.waitForURL('**/#/config/models')
  assert.equal(await sidebar.getAttribute('data-state'), 'collapsed')
  await nav.getByRole('button', { name: '主页', exact: true }).click()
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'layout and sidebar draft')
  await page.reload()
  await expandSidebar.waitFor()
  assert.equal(await sidebar.getAttribute('data-state'), 'collapsed')
  // 草稿按既有契约只在内存保留；刷新仅检查折叠偏好，重建草稿验证后续布局切换。
  await prompt.fill('layout and sidebar draft')
  await page.setViewportSize({ width: 390, height: 844 })
  const preservedContextSheet = page.locator('[data-slot="sheet-content"]')
  if (await preservedContextSheet.isVisible()) {
    await page.keyboard.press('Escape')
    await preservedContextSheet.waitFor({ state: 'hidden' })
  }
  await sidebar.waitFor({ state: 'hidden' })
  await expandSidebar.click()
  const mobileSidebar = page.locator('[data-slot="sidebar"][data-mobile="true"]')
  await mobileSidebar.waitFor()
  assert.ok(
    await mobileSidebar
      .getByTestId('workbench-home')
      .locator('span')
      .evaluate((el) => el.getBoundingClientRect().width > 20),
  )
  await mobileSidebar.getByRole('button', { name: '主页', exact: true }).click()
  await mobileSidebar.waitFor({ state: 'hidden' })
  await page.setViewportSize(desktopViewport)
  await sidebar.waitFor()
  await expandSidebar.waitFor()
  await expandSidebar.press('Enter')
  await page.waitForFunction(
    () =>
      document.querySelector('[data-slot="sidebar"][data-state]')?.getAttribute('data-state') ===
        'expanded' && document.activeElement?.id === 'workbench-sidebar-collapse',
  )
  assert.equal(await collapseSidebar.getAttribute('aria-expanded'), 'true')
  assert.equal(await prompt.inputValue(), 'layout and sidebar draft')
  report.checks.push(
    'Collapsed sidebar keeps its visible 64px icon rail at 800px and 651px, switches to a drawer at 650px, persists after reload, and the toggle retains keyboard focus without losing the draft',
  )
  await page.goto(base + '/#/config/interface?view=widgets')
  await page.locator('[data-config-card="interface-custom-ui"]').waitFor()
  assert.equal(await page.getByRole('tab', { name: '会话布局' }).count(), 0)
  await page.goto(base + '/#/chat')
  await prompt.waitFor()
  assert.equal(await prompt.inputValue(), 'layout and sidebar draft')
  assert.equal(await page.getByRole('button', { name: '会话布局' }).count(), 0)
  report.checks.push(
    'Custom component settings remain available without the removed layout editor or chat layout switcher, and navigation preserves the chat draft',
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
