import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const origin = new URL(process.argv[2] || 'http://127.0.0.1:5173').origin
assert(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(origin).hostname))
const executablePath =
  process.env.PISPER_SMOKE_BROWSER ||
  [
    chromium.executablePath(),
    join(
      homedir(),
      'Library/Caches/ms-playwright/chromium-1234/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing',
    ),
  ].find(existsSync)
assert(executablePath && existsSync(executablePath), 'Set PISPER_SMOKE_BROWSER to Chromium.')
const screenshots = await mkdtemp(join(tmpdir(), 'pisper-provider-profiles-smoke-'))
console.log(`Screenshots: ${screenshots}`)

const fixture = await build({
  stdin: {
    contents: `
      import React, { useState } from 'react'
      import { createRoot } from 'react-dom/client'
      import { useI18n } from './src/app/use-i18n.ts'
      import { QuickSetupWizard } from './src/features/config/QuickSetupWizard.tsx'
      import { ProviderConfigModal } from './src/features/config/ProviderDialogs.tsx'
      import { ConnectionList } from './src/features/config/ConnectionList.tsx'
      import { useProvidersConfig } from './src/features/config/useProvidersConfig.ts'
      function Fixture() {
        const { t } = useI18n()
        const [wizard, setWizard] = useState(false)
        const [clone, setClone] = useState(null)
        const [notice, setNotice] = useState('')
        const settings = useProvidersConfig({ notify: setNotice, requestConfirm: async () => false, t })
        const { config } = settings
        if (!config) return <div>Loading</div>
        return <main data-smoke-layout>
          <button type="button" onClick={() => setWizard(true)}>New connection</button>
          <ConnectionList
            providers={config.providers}
            defaultProviderId={config.defaultProvider}
            toggling={settings.toggling}
            settingDefault={settings.settingDefault}
            settingModel={settings.settingModel}
            onConfigure={() => {}}
            onClone={setClone}
            onSetDefault={settings.setDefaultProvider}
            onSetDefaultModel={settings.setProviderDefaultModel}
            onToggle={settings.toggleProvider}
            onDelete={settings.deleteProvider}
            onAddCustom={() => setWizard(true)}
          />
          <div role="status">{notice}</div>
          {settings.error && <div role="alert">{settings.error}</div>}
          {wizard && <QuickSetupWizard config={config}
            onClose={() => setWizard(false)}
            onCompleted={data => { settings.applyConfig(data); setWizard(false) }} />}
          {clone && <ProviderConfigModal cloneProvider={clone}
            onClose={() => setClone(null)}
            onCreated={data => { settings.applyConfig(data); setClone(null) }} />}
        </main>
      }
      createRoot(document.getElementById('root')).render(<Fixture />)
    `,
    resolveDir: root,
    loader: 'tsx',
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  loader: { '.css': 'empty' },
})
const html = `<!doctype html><html lang="zh-CN"><head>
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="stylesheet" href="/src/index.css?direct">
<style>[data-smoke-layout]{padding:16px;width:100%;max-width:1000px;margin:auto}</style>
</head><body><div id="root"></div>
<script type="module" src="/__provider-profiles-fixture.js"></script></body></html>`

const baseUrl = 'https://provider-smoke.invalid/v1'
const models = [
  { id: 'smoke-chat-a', name: 'Smoke Chat A', kind: 'chat' },
  { id: 'smoke-chat-b', name: 'Smoke Chat B', kind: 'chat' },
]
const source = {
  id: 'smoke-source',
  name: 'Smoke Source',
  type: 'chat',
  api: 'openai-responses',
  baseUrl,
  organization: 'smoke-organization',
  models,
  defaultModel: models[0].id,
  configured: true,
  enabled: true,
  custom: true,
}
let browser
try {
  browser = await chromium.launch({ executablePath, headless: true })
  for (const width of [1440, 390]) {
    // 每个视口独立内存配置；fixture 和响应均不包含来源凭据。
    const config = {
      providers: [
        structuredClone(source),
        { ...structuredClone(source), id: 'smoke-peer', name: 'Smoke Peer' },
      ],
      provider: source.id,
      defaultProvider: source.id,
      model: source.defaultModel,
      defaultModel: source.defaultModel,
      thinkingLevel: 'medium',
      toolMode: 'default',
    }
    const sourceKey = 'mock-source-secret-never-sent-to-browser'
    const keys = new Map([[source.id, sourceKey]])
    const requests = []
    const errors = []
    const unexpectedRequests = []
    const context = await browser.newContext({
      viewport: { width, height: 900 },
      locale: 'zh-CN',
      isMobile: width === 390,
      hasTouch: width === 390,
      serviceWorkers: 'block',
    })
    const page = await context.newPage()
    page.setDefaultTimeout(5_000)
    page.on('pageerror', (error) => errors.push(error.message))
    await page.route('**/*', (route) => {
      const request = route.request()
      const url = new URL(request.url())
      const method = request.method()
      if (url.origin === origin && url.pathname === '/__provider-profiles-smoke__')
        return route.fulfill({ contentType: 'text/html', body: html })
      if (url.origin === origin && url.pathname === '/__provider-profiles-fixture.js')
        return route.fulfill({ contentType: 'text/javascript', body: fixture.outputFiles[0].text })
      if (url.origin === origin && url.pathname === '/api/config' && method === 'GET')
        return route.fulfill({ json: config })
      if (
        url.origin === origin &&
        url.pathname === '/api/providers/models/refresh' &&
        method === 'POST'
      )
        return route.fulfill({ json: { config } })
      if (
        url.origin === origin &&
        method === 'POST' &&
        url.pathname === '/api/providers/models/discover-connection'
      ) {
        const body = request.postDataJSON()
        requests.push({ path: url.pathname, method, body })
        return route.fulfill({ json: { models } })
      }
      if (
        url.origin === origin &&
        method === 'POST' &&
        (url.pathname === '/api/providers' || url.pathname === `/api/providers/${source.id}/clone`)
      ) {
        const body = request.postDataJSON()
        requests.push({ path: url.pathname, method, body })
        if (config.providers.some((provider) => provider.id === body.id))
          return route.fulfill({ status: 409, json: { error: 'Duplicate mock Provider ID' } })
        const cloning = url.pathname.endsWith('/clone')
        const provider = {
          ...structuredClone(source),
          id: body.id,
          name: body.name,
          baseUrl: cloning ? source.baseUrl : body.baseUrl,
          defaultModel: cloning ? source.defaultModel : body.model,
        }
        config.providers.push(provider)
        keys.set(body.id, cloning ? body.apiKey || keys.get(source.id) : body.apiKey)
        return route.fulfill({ status: 201, json: { ...config, createdProviderId: body.id } })
      }
      if (url.origin === origin && method === 'PUT' && url.pathname === '/api/config') {
        const body = request.postDataJSON()
        requests.push({ path: url.pathname, method, body })
        const provider = config.providers.find((item) => item.id === body.provider)
        if (!provider)
          return route.fulfill({ status: 404, json: { error: 'Unknown mock Provider' } })
        if (body.model) provider.defaultModel = body.model
        if (body.setAsDefault) config.defaultProvider = config.provider = provider.id
        if (config.defaultProvider === provider.id)
          config.defaultModel = config.model = provider.defaultModel
        return route.fulfill({ json: config })
      }
      // 只放行静态样式及字体，其他请求全部阻断，绝不访问真实 API 或外部 Provider。
      if (
        url.origin === origin &&
        method === 'GET' &&
        (url.pathname === '/src/index.css' ||
          url.pathname.startsWith('/node_modules/@fontsource-variable/'))
      )
        return route.continue()
      if (url.origin === origin && url.pathname === '/favicon.ico')
        return route.fulfill({ status: 204 })
      unexpectedRequests.push(`${method} ${url.pathname}`)
      return route.abort('blockedbyclient')
    })
    const card = (name) => page.getByRole('button', { name, exact: true }).locator('..')
    const screenshot = async (name, surface) => {
      if (surface) {
        const bounds = await surface.boundingBox()
        assert(bounds && bounds.width > 0 && bounds.x >= 0 && bounds.x + bounds.width <= width + 1)
      }
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
      await page.screenshot({
        animations: 'disabled',
        path: join(screenshots, `${name}-${width}.png`),
        fullPage: true,
      })
    }
    const act = async (method, path, action) => {
      const responsePromise = page.waitForResponse(
        (response) =>
          response.url() === `${origin}${path}` && response.request().method() === method,
      )
      await action()
      const response = await responsePromise
      assert(response.ok(), `${method} ${path}: ${response.status()}`)
      return response.request().postDataJSON()
    }
    try {
      await page.goto(`${origin}/__provider-profiles-smoke__`)
      await page.getByRole('button', { name: 'Smoke Peer', exact: true }).waitFor()
      await page.evaluate(() => document.fonts.ready)
      const createdIds = []
      for (let attempt = 0; attempt < 2; attempt++) {
        await page.getByRole('button', { name: 'New connection', exact: true }).click()
        await page.getByLabel('Base URL', { exact: true }).fill(baseUrl)
        await page.getByRole('button', { name: '下一步', exact: true }).click()
        await page.getByRole('button', { name: '下一步', exact: true }).click()
        const password = page.locator('input[type="password"]')
        assert.equal(await password.count(), 1)
        assert.equal(await password.getAttribute('autocomplete'), 'new-password')
        const apiKey = `mock-typed-key-${width}-${attempt}`
        await password.fill(`  ${apiKey}  `)
        const discovered = await act('POST', '/api/providers/models/discover-connection', () =>
          page.getByRole('button', { name: '获取模型', exact: true }).click(),
        )
        assert.equal(discovered.apiKey, apiKey)
        assert.equal(discovered.providerId, '')
        assert.equal(discovered.baseUrl, baseUrl)
        assert(!Object.hasOwn(discovered, 'apiKeys'))
        await page.getByLabel('显示名称', { exact: true }).fill(`New same URL ${attempt}`)
        await page.getByLabel('模型 ID（手动输入）', { exact: true }).fill(models[1].id)
        const savedKey = `${apiKey}-latest`
        await password.fill(`  ${savedKey}  `)
        await screenshot(`wizard-${attempt}`, page.locator('.modal'))
        const saved = await act('POST', '/api/providers', () =>
          page.getByRole('button', { name: '保存修改', exact: true }).click(),
        )
        assert.equal(saved.apiKey, savedKey)
        assert.equal(saved.baseUrl, baseUrl)
        assert.equal(saved.model, models[1].id)
        assert(!Object.hasOwn(saved, 'apiKeys'))
        assert(
          saved.id !== source.id && saved.id !== 'smoke-peer' && !createdIds.includes(saved.id),
        )
        assert(saved.id.length <= 60)
        createdIds.push(saved.id)
        await password.waitFor({ state: 'detached' })
        assert.equal(config.defaultProvider, source.id)
      }
      // 打开与取消不产生创建请求；留空只发送名称和独立 ID。
      await card('Smoke Source 默认')
        .getByRole('button', { name: '克隆 Provider', exact: true })
        .click()
      const dialog = page.getByRole('dialog')
      await dialog.waitFor()
      const beforeCancel = requests.length
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      await dialog.waitFor({ state: 'detached' })
      assert.equal(requests.length, beforeCancel)
      for (const replacement of ['', `mock-replacement-key-${width}`]) {
        await card('Smoke Source 默认')
          .getByRole('button', { name: '克隆 Provider', exact: true })
          .click()
        const password = dialog.locator('input[type="password"]')
        assert.equal(await password.inputValue(), '')
        assert.equal(await password.getAttribute('placeholder'), '留空复制来源 Key')
        const name = replacement ? 'Clone replaced key' : 'Clone copied key'
        await dialog.getByLabel('显示名称', { exact: true }).fill(name)
        const id = await dialog.getByLabel('Provider ID', { exact: true }).inputValue()
        assert(id !== source.id && !createdIds.includes(id))
        if (replacement) await password.fill(`  ${replacement}  `)
        await screenshot(replacement ? 'clone-replacement' : 'clone-copy', dialog)
        const cloned = await act('POST', `/api/providers/${source.id}/clone`, () =>
          dialog.getByRole('button', { name: '克隆 Provider', exact: true }).click(),
        )
        assert.deepEqual(cloned, replacement ? { name, id, apiKey: replacement } : { name, id })
        assert(!JSON.stringify(cloned).includes(sourceKey))
        assert.equal(keys.get(id), replacement || sourceKey)
        createdIds.push(id)
        await dialog.waitFor({ state: 'detached' })
        assert.equal(config.defaultProvider, source.id)
      }
      const peer = card('Smoke Peer')
      const selector = peer.getByRole('combobox')
      const updated = await act('PUT', '/api/config', async () => {
        if (await selector.evaluate((element) => element.tagName === 'SELECT'))
          await selector.selectOption(models[1].id)
        else {
          await selector.click()
          await page.getByRole('option', { name: models[1].name, exact: true }).click()
        }
      })
      assert.deepEqual(updated, {
        provider: 'smoke-peer',
        model: models[1].id,
        setAsDefault: false,
      })
      assert.equal(config.defaultProvider, source.id)
      const switched = await act('PUT', '/api/config', () =>
        peer.getByRole('button', { name: '设为默认 Provider', exact: true }).click(),
      )
      assert.deepEqual(switched, { provider: 'smoke-peer', setAsDefault: true })
      const defaultPeer = card('Smoke Peer 默认')
      await defaultPeer.getByRole('button', { name: '设为默认 Provider', exact: true }).waitFor()
      assert(
        await defaultPeer
          .getByRole('button', { name: '设为默认 Provider', exact: true })
          .isDisabled(),
      )
      assert.equal(config.defaultModel, models[1].id)
      await screenshot('connections-default-switched', page.locator('[data-smoke-layout]'))
      assert.deepEqual(errors, [])
      assert.deepEqual(unexpectedRequests, [])
      assert.equal(requests.length, 8)
      console.log(
        `PASS ${width}px: same-URL distinct IDs, direct single-key discovery/save, clone cancel/copy/replace, provider-local model and global default requests`,
      )
    } catch (error) {
      await page
        .screenshot({ path: join(screenshots, `failure-${width}.png`), fullPage: true })
        .catch(() => {})
      console.error(
        JSON.stringify(
          {
            width,
            requests: requests.map(({ method, path, body }) => ({
              method,
              path,
              fields: Object.keys(body),
            })),
            errors,
            unexpectedRequests,
          },
          null,
          2,
        ),
      )
      throw error
    } finally {
      await context.close()
    }
  }
} finally {
  await browser?.close()
}
