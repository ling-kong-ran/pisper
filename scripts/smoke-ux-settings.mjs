// 加载失败恢复回归：真实设置页、Provider 草稿和 HTTP 请求层，不启动 Runtime。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const mocks = {
  '@/app/i18n/use-i18n':
    'const t = key => key; export function useI18n() { return {t,language:"en-US"} }',
  '@/features/config/model/provider-messages': '',
  '@/features/config/model/remote-workspace-messages': '',
  '@/lib/storage/page-state-storage': 'export const pageStateStorage = localStorage',
  '@/features/config/components/settings/QuickSetupWizard':
    'export function QuickSetupWizard() { return null }',
  '@/features/config/components/provider/ProviderDialogs':
    'export function ProviderConfigModal() { return null }',
  '@/features/config/components/provider/ProviderDiscovery':
    'export function ProviderDiscovery() { return null }',
  '@/features/config/components/settings/RuntimeSettings':
    'export function RuntimePolicySettings() { return null }',
  '@/features/config/components/settings/VisualGenerationSettings':
    'export function VisualGenerationSettings() { return null }',
}
const output = await build({
  entryPoints: [join(root, 'scripts/fixtures/ux-settings.jsx')],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  outdir: 'fixture',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  plugins: [
    {
      name: 'fixture-boundaries',
      setup(builder) {
        builder.onResolve({ filter: /.*/ }, (args) =>
          Object.hasOwn(mocks, args.path) ? { path: args.path, namespace: 'mock' } : null,
        )
        builder.onLoad({ filter: /.*/, namespace: 'mock' }, (args) => ({
          contents: mocks[args.path],
          loader: 'js',
        }))
      },
    },
  ],
})
const js = output.outputFiles.find((file) => file.path.endsWith('.js')).text
const server = createServer((request, response) => {
  response.setHeader('Content-Type', request.url === '/bundle.js' ? 'text/javascript' : 'text/html')
  response.end(
    request.url === '/bundle.js'
      ? js
      : '<!doctype html><html><body><div id="root"></div><script type="module" src="/bundle.js"></script></body></html>',
  )
})
const config = {
  provider: 'fixture',
  model: 'fixture-model',
  thinkingLevel: 'off',
  toolMode: 'full',
  providers: [
    {
      id: 'fixture',
      name: 'Fixture provider',
      type: 'chat',
      api: 'openai-responses',
      baseUrl: 'http://fixture.invalid/v1',
      configured: true,
      enabled: true,
      defaultModel: 'fixture-model',
      models: [{ id: 'fixture-model', name: 'Fixture model', kind: 'chat' }],
    },
  ],
}
const discovery = { providers: [], errors: [] }
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
  assert(
    existsSync(executablePath),
    'Install Chromium with npx playwright-core install chromium, or set PISPER_SMOKE_BROWSER.',
  )
  browser = await chromium.launch({ headless: true, executablePath })
  const page = await browser.newPage({ viewport: { width: 1100, height: 900 } })
  page.setDefaultTimeout(10000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const requests = new Map()
  const held = new Map()
  const holds = new Set()
  const failures = new Set(['/api/config', '/api/providers/models/refresh'])
  let remoteEnabled = false
  const responseFor = (path) => {
    if (path === '/api/config') return config
    if (path === '/api/providers/models/refresh') return { config }
    if (path === '/api/providers/import-local') return { discovery, imported: [], skipped: [] }
    if (path === '/api/remote/status')
      return { apiVersion: 1, enabled: remoteEnabled, listening: false, error: null }
    if (path === '/api/remote/enabled') return { enabled: remoteEnabled }
    if (path === '/api/remote/devices')
      return {
        devices: [
          {
            id: 'fixture-device',
            name: 'Fixture phone',
            lastSeenAt: '2026-01-01T00:00:00Z',
            revokedAt: null,
          },
        ],
      }
    if (path === '/api/remote/pairing-requests') return { requests: [] }
    if (path === '/api/remote/firewall') return { state: 'disabled', busy: false, checkedAt: null }
    throw new Error('Unexpected fixture API ' + path)
  }
  const fulfill = (route, path) =>
    route.fulfill({
      status: failures.has(path) ? 503 : 200,
      contentType: 'application/json',
      body: JSON.stringify(
        failures.has(path) ? { error: 'Fixture service unavailable' } : responseFor(path),
      ),
    })
  await page.route('**/api/**', async (route) => {
    const path = new URL(route.request().url()).pathname
    requests.set(path, (requests.get(path) || 0) + 1)
    if (path === '/api/remote/enabled') remoteEnabled = route.request().postDataJSON().enabled
    if (holds.has(path)) {
      held.set(path, route)
      return
    }
    await fulfill(route, path)
  })
  const origin = `http://127.0.0.1:${server.address().port}`
  const retry = () => page.getByRole('button', { name: 'config:loadState.retry', exact: true })
  const retrying = () =>
    page.getByRole('button', { name: 'config:loadState.retrying', exact: true })
  const release = async (path) => {
    assert.ok(held.has(path), 'Expected held request: ' + path)
    const route = held.get(path)
    held.delete(path)
    holds.delete(path)
    await fulfill(route, path)
  }
  const beginRetry = async (path) => {
    const before = requests.get(path) || 0
    holds.add(path)
    const sent = page.waitForRequest((request) => new URL(request.url()).pathname === path)
    // 同一事件循环内连点，覆盖 React 禁用状态尚未提交的窗口。
    await retry().evaluate((button) => {
      button.click()
      button.click()
    })
    await sent
    await retrying().waitFor()
    assert.equal(await retrying().isDisabled(), true)
    assert.equal(requests.get(path), before + 1, 'A retry must not create duplicate requests')
  }

  await page.goto(origin)
  await retry().waitFor()
  assert.equal(
    await page.getByText('config:configPage.loadingModelCatalog', { exact: true }).count(),
    0,
  )
  await page.getByText('config:loadState.failed', { exact: true }).waitFor()
  failures.delete('/api/config')
  await beginRetry('/api/config')
  await release('/api/config')
  await page.getByRole('heading', { name: 'Fixture provider', exact: true }).waitFor()
  await retry().waitFor()
  await page.getByText('config:loadState.stale', { exact: true }).waitFor()
  const name = page.getByRole('textbox', { name: 'config:configPage.displayName', exact: true })
  await name.fill('Unsaved connection draft')
  failures.add('/api/config')
  await beginRetry('/api/config')
  assert.equal(await name.inputValue(), 'Unsaved connection draft')
  await release('/api/config')
  await retry().waitFor()
  assert.equal(await name.inputValue(), 'Unsaved connection draft')
  failures.clear()
  await retry().click()
  await page.getByText('config:loadState.stale', { exact: true }).waitFor({ state: 'detached' })
  assert.equal(await name.inputValue(), 'Unsaved connection draft')
  console.log(
    'PASS: model initial failure, retry coalescing, stale data and unsaved draft retention',
  )

  failures.add('/api/remote/status')
  await page.goto(origin + '/?page=remote')
  await retry().waitFor()
  assert.equal(await page.getByText('config:loadState.loading', { exact: true }).count(), 0)
  failures.clear()
  await beginRetry('/api/remote/status')
  await release('/api/remote/status')
  await page.getByText('Fixture phone', { exact: true }).waitFor()
  const refresh = page.getByRole('button', { name: 'config:remoteAccess.refresh', exact: true })
  failures.add('/api/remote/status')
  await refresh.click()
  await retry().waitFor()
  await page.getByText('config:loadState.stale', { exact: true }).waitFor()
  assert.equal(await page.getByText('Fixture phone', { exact: true }).isVisible(), true)
  await beginRetry('/api/remote/status')
  assert.equal(await page.getByText('Fixture phone', { exact: true }).isVisible(), true)
  assert.equal(await refresh.isDisabled(), true)
  failures.clear()
  await release('/api/remote/status')
  await page.getByText('config:loadState.stale', { exact: true }).waitFor({ state: 'detached' })
  assert.equal(await page.getByText('Fixture phone', { exact: true }).isVisible(), true)
  console.log('PASS: remote access initial failure, retry coalescing and stale device retention')

  holds.add('/api/remote/status')
  const refreshing = page.waitForRequest(
    (request) => new URL(request.url()).pathname === '/api/remote/status',
  )
  await refresh.click()
  await refreshing
  const cancelledRefresh = page.waitForEvent('requestfailed', {
    predicate: (request) => new URL(request.url()).pathname === '/api/remote/status',
  })
  holds.delete('/api/remote/status')
  held.delete('/api/remote/status')
  const toggle = page.getByRole('switch', { name: 'config:remoteAccess.title', exact: true })
  await toggle.click()
  await cancelledRefresh
  await page.waitForFunction(
    () => document.querySelector('[role="switch"]')?.getAttribute('aria-checked') === 'true',
  )
  assert.equal(await toggle.isEnabled(), true)
  console.log('PASS: committed remote changes replace an older in-flight refresh')

  for (const [query, path] of [
    ['', '/api/config'],
    ['?page=remote', '/api/remote/status'],
  ]) {
    holds.add(path)
    const started = page.waitForRequest((request) => new URL(request.url()).pathname === path)
    await page.goto(origin + '/' + query)
    await started
    const cancelled = page.waitForEvent('requestfailed', {
      predicate: (request) => new URL(request.url()).pathname === path,
    })
    await page.evaluate(() => window.settingsFixture.setMounted(false))
    await cancelled
    await page.getByText('Fixture unmounted', { exact: true }).waitFor()
    holds.delete(path)
    held.delete(path)
    await page.evaluate(() => window.settingsFixture.setMounted(true))
    await page
      .getByText(query ? 'Fixture phone' : 'Fixture provider', { exact: true })
      .first()
      .waitFor()
  }
  console.log('PASS: leaving either settings page aborts owned reads and remount can load normally')
  assert.deepEqual(errors, [])
} finally {
  await browser?.close()
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
}
