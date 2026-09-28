import assert from 'node:assert/strict'
import { mkdtemp, mkdir, rm } from 'node:fs/promises'
import { createServer } from 'node:net'
import { join } from 'node:path'
import { DEFAULT_BRANCH } from '../shared/app-update.mjs'

async function reservePort(port = 0) {
  const server = createServer()
  await new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(port, '127.0.0.1', resolve)
  })
  return server
}

async function closePort(server) {
  if (server?.listening)
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    )
}

async function freeDevPort() {
  // 开发 Runtime 的 HMR 使用 HTTP 端口 + 1，不能传 0 让 HMR 意外监听特权端口 1。
  for (let attempt = 0; attempt < 10; attempt++) {
    const http = await reservePort()
    let hmr
    try {
      const port = http.address().port
      if (port === 65535) continue
      hmr = await reservePort(port + 1)
      return port
    } catch (error) {
      if (error.code !== 'EADDRINUSE') throw error
    } finally {
      await closePort(hmr)
      await closePort(http)
    }
  }
  throw new Error('Unable to reserve loopback HTTP/HMR ports for first-run verification')
}

async function verifyDialogBounds(page, dialog) {
  const viewport = page.viewportSize()
  const bounds = await dialog.boundingBox()
  assert.ok(bounds, 'First-run dialog must be visible')
  assert.ok(bounds.x >= 0 && bounds.x + bounds.width <= viewport.width + 1)
  assert.ok(bounds.y >= 0 && bounds.y + bounds.height <= viewport.height + 1)
  const close = dialog.getByRole('button', { name: '关闭对话框', exact: true })
  const closeBounds = await close.boundingBox()
  assert.ok(
    closeBounds && closeBounds.y >= 0 && closeBounds.y + closeBounds.height <= viewport.height,
  )
  assert.ok(
    await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    'First-run overlay must not introduce horizontal overflow',
  )
  return close
}

export async function verifyFirstRunSetup({
  browser,
  root,
  output,
  modelBaseUrl,
  modelId,
  report,
}) {
  const modelEndpoint = new URL(modelBaseUrl)
  assert.equal(modelEndpoint.protocol, 'http:')
  assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(modelEndpoint.hostname))
  assert.equal(modelEndpoint.username + modelEndpoint.password, '')
  assert.ok(typeof modelId === 'string' && modelId)
  const temporary = await mkdtemp(join(output, 'first-run-'))
  const dataDir = join(temporary, 'agent')
  const workspace = join(temporary, 'workspace')
  await mkdir(workspace)
  const previousAgentDir = process.env.PI_CODING_AGENT_DIR
  const contexts = []
  const errors = []
  const failedApi = []
  const forbiddenRequests = []
  let runtime
  let page
  let stage = 'startup'
  async function cleanup() {
    try {
      const closedContexts = await Promise.allSettled(contexts.map((context) => context.close()))
      await runtime?.close()
      // 等待 Runtime 释放持久化队列和监听器后才删除数据，截图保存在 temporary 之外。
      await rm(temporary, { recursive: true, force: true })
      const failures = closedContexts.filter((result) => result.status === 'rejected')
      if (failures.length)
        throw new AggregateError(
          failures.map((result) => result.reason),
          'Browser cleanup failed',
        )
    } finally {
      if (previousAgentDir === undefined) delete process.env.PI_CODING_AGENT_DIR
      else process.env.PI_CODING_AGENT_DIR = previousAgentDir
    }
  }
  try {
    // 单独启动开发中间件，既不复用主 smoke 的已配置数据，也不连接用户的 dev 服务。
    const { createPisperRuntime } = await import('../runtime/app-runtime.mjs')
    runtime = await createPisperRuntime({
      root,
      runtimeCwd: workspace,
      dataDir,
      production: false,
      port: await freeDevPort(),
      host: '127.0.0.1',
      remote: { enabled: false },
    })
    const base = runtime.url
    async function resetOnboardingPreference() {
      const response = await fetch(base + '/api/local/browser-preferences', {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json', Origin: base },
        body: JSON.stringify({ updates: { 'pisper-model-onboarding-v1-dismissed': null } }),
      })
      assert.equal(response.status, 204)
    }
    async function config() {
      const response = await fetch(base + '/api/config', {
        headers: { Origin: base },
        signal: AbortSignal.timeout(30000),
      })
      assert.ok(response.ok, `First-run config API returned ${response.status}`)
      return response.json()
    }
    const visibleProviders = (data) =>
      data.providers.filter((entry) => entry.configured || entry.custom)
    assert.equal(
      visibleProviders(await config()).length,
      0,
      'Runtime must start without connections',
    )
    async function freshPage(viewport = { width: 1440, height: 900 }) {
      const context = await browser.newContext({ viewport, locale: 'zh-CN' })
      contexts.push(context)
      context.setDefaultTimeout(30000)
      context.setDefaultNavigationTimeout(60000)
      await context.addInitScript(() => {
        localStorage.setItem('pisper-language', 'zh-CN')
        localStorage.setItem('pisper-theme', 'light')
      })
      // 只固定外部发现/更新；配置、模型发现、创建连接和渠道读取都经过真实 Runtime。
      await context.route('**/api/providers/discovery', (route) =>
        route.fulfill({ json: { providers: [], errors: [] } }),
      )
      await context.route('**/api/providers/import-local', async (route) =>
        route.fulfill({
          json: {
            config: await config(),
            discovery: { providers: [], errors: [] },
            imported: [],
            skipped: [],
          },
        }),
      )
      await context.route('**/api/app-update*', (route) =>
        route.fulfill({
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
            message: 'Local UI fixture: update checks are verified separately.',
          },
        }),
      )
      const next = await context.newPage()
      next.on('pageerror', (error) => errors.push(error.message))
      next.on('response', (response) => {
        const path = new URL(response.url()).pathname
        if (path.startsWith('/api/') && response.status() >= 400)
          failedApi.push({ path, status: response.status() })
      })
      next.on('request', (request) => {
        const path = new URL(request.url()).pathname
        if (
          (path === '/api/chat' && request.method() === 'POST') ||
          (path.startsWith('/api/channels') && !['GET', 'HEAD'].includes(request.method()))
        )
          forbiddenRequests.push({ path, method: request.method() })
      })
      return next
    }
    const onboarding = () => page.getByRole('dialog', { name: '先连接一个模型', exact: true })
    const chatReady = () => page.getByRole('textbox', { name: '任务描述', exact: true }).waitFor()

    stage = 'desktop-dismiss'
    page = await freshPage()
    await page.goto(base + '/#/chat')
    await onboarding().waitFor()
    await verifyDialogBounds(page, onboarding())
    await page.screenshot({ path: join(output, 'first-run-desktop.png') })
    await page.keyboard.press('Escape')
    await onboarding().waitFor({ state: 'hidden' })
    await page.reload()
    await chatReady()
    assert.equal(
      await onboarding().count(),
      0,
      'Dismissed onboarding must stay closed after reload',
    )
    report.checks.push(
      'Fresh install onboarding can be dismissed with Escape and stays dismissed after reload',
    )
    page = await freshPage()
    await page.goto(base + '/#/chat')
    await chatReady()
    assert.equal(await onboarding().count(), 0)
    report.checks.push('Onboarding dismissal also survives a fresh WebView storage origin')

    stage = 'mobile-dismiss'
    await resetOnboardingPreference()
    page = await freshPage({ width: 390, height: 844 })
    await page.goto(base + '/#/chat')
    await onboarding().waitFor()
    const close = await verifyDialogBounds(page, onboarding())
    await page.screenshot({ path: join(output, 'first-run-mobile.png') })
    await close.click()
    await onboarding().waitFor({ state: 'hidden' })
    await page.reload()
    await chatReady()
    assert.equal(await onboarding().count(), 0)
    report.checks.push(
      '390px first-run overlay fits the viewport, exposes close control and persists dismissal',
    )

    stage = 'empty-providers'
    await resetOnboardingPreference()
    page = await freshPage()
    await page.goto(base + '/#/chat')
    await onboarding().waitFor()
    await onboarding().getByRole('button', { name: '设置模型', exact: true }).click()
    const wizard = page.getByRole('dialog', { name: '快速配置模型', exact: true })
    await wizard.waitFor()
    assert.match(page.url(), /#\/config\/models/)
    await wizard.getByRole('button', { name: '关闭对话框', exact: true }).click()
    await wizard.waitFor({ state: 'hidden' })
    const connections = page.locator('[data-model-provider-split-panel]')
    await connections.getByText('尚未添加供应商', { exact: true }).waitFor()
    assert.equal(
      await connections.getByRole('navigation', { name: '连接', exact: true }).count(),
      0,
    )
    const quickSetup = page
      .locator('main > header')
      .getByRole('button', { name: '快速设置', exact: true })
    // 开发模式下页面和应用标题栏分开懒加载，等标题栏就绪后再检查入口数量。
    await quickSetup.waitFor()
    assert.equal(await page.getByRole('button', { name: '快速设置', exact: true }).count(), 1)
    assert.equal(await quickSetup.count(), 1)
    await page.screenshot({ path: join(output, 'first-run-empty-providers.png') })
    report.checks.push(
      'Fresh browser onboarding opens Quick Setup; zero connections hide connection navigation and retain a single header action',
    )

    stage = 'first-connection'
    await quickSetup.click()
    await wizard.getByLabel('Base URL', { exact: true }).fill(modelBaseUrl)
    await wizard.getByRole('button', { name: '下一步', exact: true }).click()
    await wizard.getByRole('combobox').click()
    await page.getByRole('option', { name: 'OpenAI Chat Completions', exact: true }).click()
    await wizard.getByRole('button', { name: '下一步', exact: true }).click()
    const connectionName = 'First-run local fixture'
    await wizard.getByLabel('显示名称', { exact: true }).fill(connectionName)
    await wizard.getByLabel('API Key', { exact: true }).fill('local-ui-fixture-key')
    const discovered = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname === '/api/providers/models/discover-connection',
    )
    await wizard.getByRole('button', { name: '获取模型', exact: true }).click()
    assert.ok((await discovered).ok(), 'Quick Setup must discover models from the loopback fixture')
    await wizard.getByRole('button', { name: modelId, exact: true }).click()
    assert.equal(
      await wizard.getByLabel('模型 ID（手动输入）', { exact: true }).inputValue(),
      modelId,
    )
    await wizard.getByRole('button', { name: '保存修改', exact: true }).click()
    await wizard.waitFor({ state: 'hidden' })
    const saved = await config()
    const [created] = visibleProviders(saved)
    assert.equal(visibleProviders(saved).length, 1)
    assert.equal(created.name, connectionName)
    assert.equal(created.configured, true)
    assert.equal(created.enabled, true)
    assert.equal(created.defaultModel, modelId)
    assert.equal(saved.defaultProvider || saved.provider, created.id)
    assert.equal(saved.defaultModel || saved.model, modelId)
    const providerButton = connections
      .getByRole('navigation', { name: '连接', exact: true })
      .getByRole('button', { name: connectionName, exact: true })
    await providerButton.getByText('默认', { exact: true }).waitFor()
    await connections.getByText('默认供应商', { exact: true }).waitFor()
    await page.screenshot({ path: join(output, 'first-run-configured.png') })
    report.checks.push(
      'Real Quick Setup discovers and saves the first loopback connection as default, with visible default badges',
    )

    stage = 'configured-startup-and-channels'
    page = await freshPage()
    await page.goto(base + '/#/chat')
    await chatReady()
    assert.equal(
      await onboarding().count(),
      0,
      'Configured model suppresses onboarding in a fresh browser',
    )
    await page.reload()
    await chatReady()
    assert.equal(await onboarding().count(), 0)
    await page.goto(base + '/#/channels')
    await page.getByRole('heading', { name: '飞书应用机器人', exact: true }).waitFor()
    await page.getByRole('heading', { name: 'Telegram 机器人', exact: true }).waitFor()
    assert.equal(
      await page.getByText('channels:channelsPage.feishuAppBot', { exact: true }).count(),
      0,
    )
    await page.screenshot({ path: join(output, 'first-run-dev-channels.png') })
    assert.deepEqual(errors, [], 'Dev first-run and channel routes must not raise page errors')
    assert.deepEqual(failedApi, [], 'First-run UI must complete without API errors')
    assert.deepEqual(forbiddenRequests, [], 'Verification must never send chat or connect channels')
    report.checks.push(
      'Configured users skip onboarding without a dismissal flag; dev channels lazy-load localized content without errors',
    )
  } catch (error) {
    if (page && !page.isClosed()) {
      try {
        const startupState = await page.evaluate(() => ({
          dismissed: localStorage.getItem('pisper-model-onboarding-v1-dismissed') === '1',
          route: location.hash.split('?')[0],
          onboardingVisible: Boolean(document.querySelector('[data-slot="dialog-content"]')),
        }))
        report.checks.push(`First-run failure state: ${JSON.stringify(startupState)}`)
        await page.screenshot({ path: join(output, `first-run-${stage}-failure.png`) })
      } catch (captureError) {
        report.checks.push(`First-run failure screenshot unavailable: ${captureError.name}`)
      }
    }
    report.pageErrors.push(...errors)
    report.failedApi.push(...failedApi)
    throw error
  } finally {
    await cleanup()
  }
}
