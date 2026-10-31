import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { existsSync } from 'node:fs'
import { mkdtemp, readdir, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { EventEmitter, once } from 'node:events'
import { chromium } from 'playwright-core'
import { SMOKE_BROWSER } from './fixtures/smoke-browser.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const output = await mkdtemp(join(tmpdir(), 'pisper-notifications-'))
const bundle = await build({
  entryPoints: [join(root, 'scripts/fixtures/replica-notifications.tsx')],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  loader: { '.css': 'empty' },
})
const cssNames = (await readdir(join(root, 'dist/assets')))
  .filter((name) => name.endsWith('.css'))
  .sort()
const css = (
  await Promise.all(cssNames.map((name) => readFile(join(root, 'dist/assets', name), 'utf8')))
).join('\n')
const server = createServer((request, response) => {
  const path = new URL(request.url, 'http://fixture').pathname
  response.setHeader(
    'Content-Type',
    path === '/fixture.js' ? 'text/javascript' : path === '/fixture.css' ? 'text/css' : 'text/html',
  )
  response.end(
    path === '/fixture.js'
      ? bundle.outputFiles[0].text
      : path === '/fixture.css'
        ? css
        : '<!doctype html><html lang="zh-CN"><head><meta name="viewport" content="width=device-width,initial-scale=1"/><link rel="stylesheet" href="/fixture.css"/></head><body><div id="root"></div><script type="module" src="/fixture.js"></script></body></html>',
  )
})
const platforms = ['browser', 'feishu', 'weixin', 'qq', 'telegram']
const events = [
  'chat.completed',
  'chat.waiting',
  'schedule.completed',
  'schedule.failed',
  'workflow.completed',
  'workflow.failed',
]
const initialData = () => ({
  browser: { enabled: true },
  connections: {
    feishu: { enabled: true, status: 'connected' },
    weixin: null,
    qq: null,
    telegram: null,
  },
  scopes: [{ platform: 'feishu', peerId: 'fixture' }],
  templates: events.map((id) => ({
    id,
    name: id,
    description: id,
    enabled: true,
    variables: ['chat.title'],
    channels: Object.fromEntries(
      platforms.map((platform) => [
        platform,
        { content: `${id}/${platform} original`, enabled: true },
      ]),
    ),
  })),
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath =
    SMOKE_BROWSER ||
    [
      chromium.executablePath(),
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    ].find(existsSync)
  assert(executablePath, 'Set PISPER_SMOKE_BROWSER to an installed browser.')
  browser = await chromium.launch({ executablePath, headless: true })
  const page = await browser.newPage({
    viewport: { width: 1000, height: 900 },
    reducedMotion: 'reduce',
  })
  page.setDefaultTimeout(4000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const url = `http://127.0.0.1:${server.address().port}`
  const externalRequests = []
  await page.route('**/*', (route) => {
    if (new URL(route.request().url()).origin !== url) {
      externalRequests.push(route.request().url())
      return route.abort()
    }
    return route.fallback()
  })
  let data = initialData()
  let failLoad = true
  let holdWrites = false
  let legacy = false
  let emptyCatalog = false
  let failTest = false
  const writes = []
  const waiting = []
  const writeEvents = new EventEmitter()
  const waitForWrites = async (count) => {
    while (writes.length < count) await once(writeEvents, 'write')
  }
  const tests = []
  await page.route('**/api/settings/notifications**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    if (request.method() === 'GET') {
      if (failLoad) return route.fulfill({ status: 503, json: { error: 'Fixture unavailable' } })
      const result = structuredClone(data)
      if (emptyCatalog) result.templates = []
      if (legacy)
        for (const template of result.templates)
          for (const channel of Object.values(template.channels)) delete channel.enabled
      return route.fulfill({ json: result })
    }
    const parts = path.split('/')
    const event = decodeURIComponent(parts[5] || '')
    const platform = parts[6]
    if (path.endsWith('/test')) {
      tests.push({ event, platform })
      if (failTest) return route.fulfill({ status: 503, json: { error: 'Fixture test rejected' } })
      return route.fulfill({
        json: {
          sent: 1,
          title: 'Saved template',
          body: data.templates.find((item) => item.id === event).channels[platform].content,
        },
      })
    }
    const body = request.postDataJSON()
    const entry = { event, platform, body }
    writes.push(entry)
    let fail = false
    if (holdWrites)
      fail = await new Promise((resolve) => {
        waiting.push({ entry, release: resolve })
        writeEvents.emit('write')
      })
    else writeEvents.emit('write')
    if (fail) return route.fulfill({ status: 503, json: { error: 'Fixture write rejected' } })
    if (path.endsWith('/browser') && !path.includes('/templates/'))
      data.browser.enabled = body.enabled
    else {
      const template = data.templates.find((item) => item.id === event)
      if ('enabled' in body) template.enabled = body.enabled
      if ('channelEnabled' in body) template.channels[platform].enabled = body.channelEnabled
      if ('content' in body) template.channels[platform].content = body.content.trim()
    }
    return route.fulfill({ json: structuredClone(data) })
  })
  await page.goto(url)
  await page.getByRole('button', { name: /重试|Retry/, exact: true }).waitFor()
  failLoad = false
  await page.getByRole('button', { name: /重试|Retry/, exact: true }).click()
  const row = page.locator('[data-notification-event="chat.completed"]')
  await row.waitFor()
  assert.equal(await page.locator('[data-notification-event]').count(), 6)
  console.log('PASS initial failure is retryable and all six real event rows load')

  holdWrites = true
  await row.getByRole('button').click()
  const browserOption = page.getByRole('menuitemcheckbox', { name: /系统通知|System notification/ })
  await browserOption.click()
  await waitForWrites(1)
  assert.equal(await browserOption.getAttribute('aria-checked'), 'false')
  await browserOption.click()
  assert.equal(await browserOption.getAttribute('aria-checked'), 'true')
  assert.equal(
    await page.evaluate(
      () => window.notificationEvidence.snapshots.at(-1).templates[0].channels.browser.enabled,
    ),
    true,
    'Optimistic state must not reach App projection',
  )
  waiting.shift().release(true)
  await waitForWrites(2)
  assert.equal(await browserOption.getAttribute('aria-checked'), 'true')
  waiting.shift().release(false)
  await page.waitForFunction(
    () => !document.querySelector('[data-notification-event="chat.completed"] [aria-busy="true"]'),
  )
  assert.equal(
    await browserOption.getAttribute('aria-checked'),
    'true',
    'An older failure cannot roll back the latest intent',
  )
  holdWrites = false
  await browserOption.click()
  await page.waitForFunction(
    () =>
      window.notificationEvidence.snapshots.at(-1).templates[0].channels.browser.enabled === false,
  )
  assert.deepEqual(writes.at(-1).body, { channelEnabled: false })
  holdWrites = true
  const beforeParallel = writes.length
  await browserOption.click()
  const feishuOption = page.getByRole('menuitemcheckbox', { name: '飞书', exact: true })
  await feishuOption.click()
  await waitForWrites(beforeParallel + 2)
  waiting
    .splice(
      waiting.findIndex(({ entry }) => entry.platform === 'feishu'),
      1,
    )[0]
    .release(false)
  await page.waitForFunction(
    () =>
      window.notificationEvidence.snapshots.at(-1).templates[0].channels.feishu.enabled === false,
  )
  waiting.shift().release(true)
  await page.waitForFunction(
    () => !document.querySelector('[data-notification-event="chat.completed"] [aria-busy="true"]'),
  )
  assert.equal(
    await feishuOption.getAttribute('aria-checked'),
    'false',
    'A failure in another channel must not undo the saved channel',
  )
  assert.equal(await browserOption.getAttribute('aria-checked'), 'false')
  await page.keyboard.press('Escape')
  holdWrites = false
  await page.getByRole('button', { name: /重试|Retry/, exact: true }).click()
  await page.waitForFunction(
    () =>
      window.notificationEvidence.snapshots.at(-1).templates[0].channels.browser.enabled === true,
  )
  assert.equal(data.templates[0].channels.feishu.enabled, false)
  await row.getByRole('button').click()
  await page
    .getByRole('menuitemcheckbox', { name: /启用此事件|Enable this event/, exact: true })
    .click()
  await page.waitForFunction(
    () => window.notificationEvidence.snapshots.at(-1).templates[0].enabled === false,
  )
  assert.deepEqual(writes.at(-1).body, { enabled: false })
  assert.equal(data.templates[0].channels.feishu.enabled, false)
  await page
    .getByRole('menuitemcheckbox', { name: /启用此事件|Enable this event/, exact: true })
    .click()
  await page.waitForFunction(
    () => window.notificationEvidence.snapshots.at(-1).templates[0].enabled === true,
  )
  await page.keyboard.press('Escape')
  console.log(
    'PASS real channel payload, non-closing menu, latest intent, and confirmed-only App projection',
  )

  await page.getByRole('button', { name: /编辑通知模板|Edit notification templates/ }).click()
  const content = page.getByRole('textbox', { name: /消息内容|Message content/ })
  await content.fill('Draft A')
  assert.equal(
    await page.getByRole('button', { name: /测试发送|Send test/, exact: true }).isDisabled(),
    true,
  )
  const channel = page.getByRole('combobox', { name: /通知渠道|Notification channel/, exact: true })
  await channel.click()
  await page.getByRole('option', { name: '飞书', exact: true }).click()
  await content.fill('Draft B')
  await channel.click()
  await page.getByRole('option', { name: '系统通知', exact: true }).click()
  assert.equal(await content.inputValue(), 'Draft A')
  holdWrites = true
  const save = page.getByRole('button', { name: /保存模板|Save template/, exact: true })
  const beforeSave = writes.length
  await save.click()
  await waitForWrites(beforeSave + 1)
  await content.fill('Draft A newer')
  waiting.shift().release(false)
  await page.waitForFunction(
    () => !document.querySelector('[data-template-save][aria-busy="true"]'),
  )
  assert.equal(await content.inputValue(), 'Draft A newer')
  assert.equal(
    await page.getByRole('button', { name: /测试发送|Send test/, exact: true }).isDisabled(),
    true,
  )
  const beforeFailedSave = writes.length
  await save.click()
  await waitForWrites(beforeFailedSave + 1)
  waiting.shift().release(true)
  await page.getByRole('alert').filter({ hasText: 'Fixture write rejected' }).waitFor()
  assert.equal(await content.inputValue(), 'Draft A newer')
  holdWrites = false
  await save.click()
  await page.waitForFunction(
    () => document.querySelector('[data-template-save]')?.disabled === true,
  )
  failTest = true
  await page.getByRole('button', { name: /测试发送|Send test/, exact: true }).click()
  await page.getByRole('alert').filter({ hasText: 'Fixture test rejected' }).waitFor()
  assert.equal(await page.evaluate(() => window.notificationEvidence.systemTests.length), 0)
  failTest = false
  await page.getByRole('button', { name: /测试发送|Send test/, exact: true }).click()
  assert.equal(tests.at(-1).platform, 'browser')
  await page.waitForFunction(() => window.notificationEvidence.systemTests.length > 0)
  assert.equal(
    await page.evaluate(() => window.notificationEvidence.systemTests.at(-1).body),
    'Draft A newer',
  )
  console.log(
    'PASS per-channel drafts, stale-save protection, failed save/test retries, and testing the saved version only',
  )
  await page.screenshot({
    path: join(output, 'notifications-templates.png'),
    fullPage: true,
    animations: 'disabled',
  })

  legacy = true
  await page.goto(url)
  await page.locator('[data-notification-event="chat.completed"]').getByRole('button').click()
  assert.equal(
    await page
      .getByRole('menuitemcheckbox', { name: /系统通知|System notification/ })
      .getAttribute('aria-disabled'),
    'true',
  )
  console.log(
    'PASS old Runtime keeps existing settings while refusing unsupported channel preferences',
  )
  await page.screenshot({
    path: join(output, 'notifications-legacy.png'),
    fullPage: true,
    animations: 'disabled',
  })
  legacy = false
  emptyCatalog = true
  await page.goto(url)
  await page
    .getByRole('status')
    .filter({ hasText: /没有可用的通知事件|No notification events/ })
    .waitFor()
  assert.equal(await page.locator('[data-notification-event]').count(), 0)
  console.log('PASS empty catalog is explicit without invented editable events')
  emptyCatalog = false
  legacy = false
  await page.goto(url)
  await page.locator('[data-notification-event="chat.completed"]').getByRole('button').click()
  holdWrites = true
  const beforeUnmount = writes.length
  await page.getByRole('menuitemcheckbox', { name: /系统通知|System notification/ }).click()
  await waitForWrites(beforeUnmount + 1)
  await page.getByRole('menuitemcheckbox', { name: /系统通知|System notification/ }).click()
  await page.keyboard.press('Escape')
  const projectionCount = await page.evaluate(() => window.notificationEvidence.snapshots.length)
  const aborted = page.waitForEvent('requestfailed', {
    predicate: (request) => request.method() === 'PUT',
  })
  await page.getByRole('button', { name: 'Unmount notifications' }).click()
  await aborted
  waiting.shift().release(false)
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(writes.length, beforeUnmount + 1)
  assert.equal(
    await page.evaluate(() => window.notificationEvidence.snapshots.length),
    projectionCount,
  )
  console.log(
    'PASS unmount aborts active settings writes, drops queued intent, and suppresses stale projection',
  )
  await writeFile(join(output, 'results.json'), JSON.stringify({ writes, tests, errors }, null, 2))
  assert.deepEqual(errors, [])
  assert.deepEqual(externalRequests, [])
  console.log(`ARTIFACTS ${output}`)
} finally {
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
