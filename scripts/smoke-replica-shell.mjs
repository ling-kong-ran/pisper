import assert from 'node:assert/strict'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { chromium } from 'playwright-core'

const output = await mkdtemp(join(tmpdir(), 'pisper-replica-shell-'))
const server = await createServer({
  configFile: false,
  cacheDir: join(output, 'vite-cache'),
  plugins: [
    react(),
    tailwindcss(),
    {
      name: 'replica-shell-fixture',
      configureServer(server) {
        server.middlewares.use(async (request, response, next) => {
          if (request.url !== '/__shell') return next()
          response.setHeader('Content-Type', 'text/html')
          response.end(
            await server.transformIndexHtml(
              '/__shell',
              '<!doctype html><html lang="zh-CN"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/replica-shell.tsx"></script></body></html>',
            ),
          )
        })
      },
    },
  ],
  resolve: { alias: { '@': `${process.cwd()}/src`, '@shared': `${process.cwd()}/shared` } },
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
})
let browser
try {
  await server.listen()
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_SMOKE_BROWSER
      ? { executablePath: process.env.PISPER_SMOKE_BROWSER }
      : {}),
  })
  const page = await browser.newPage({
    viewport: { width: 1200, height: 800 },
    locale: 'zh-CN',
    timezoneId: 'Asia/Shanghai',
    deviceScaleFactor: 1,
  })
  // 固定相对时间文案，但不冻结定时器，让防抖、动画与请求时序保持真实。
  await page.clock.setFixedTime(new Date('2026-10-20T09:00:00Z'))
  page.setDefaultTimeout(6000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const sessions = [
    {
      id: 'alpha',
      name: 'Alpha conversation',
      firstMessage: 'fixture',
      modified: '2026-10-20T00:00:00Z',
    },
    { id: 'fail', name: 'Failure conversation', modified: '2026-10-19T00:00:00Z' },
  ]
  let failLabels = false
  let releaseLabels
  let signalLabelRequest
  const labelRequestStarted = new Promise((resolve) => {
    signalLabelRequest = resolve
  })
  let holdLabels = false
  let labelRequests = 0
  await page.route('**/api/**', async (route) => {
    const url = new URL(route.request().url())
    if (url.pathname === '/api/sessions') return route.fulfill({ json: { sessions } })
    if (url.pathname === '/api/session-labels') {
      labelRequests++
      if (holdLabels)
        await new Promise((resolve) => {
          releaseLabels = resolve
          signalLabelRequest()
        })
      return route.fulfill(
        failLabels
          ? { status: 503, json: { error: 'Fixture unavailable' } }
          : {
              json: {
                labels: [
                  {
                    sessionId: 'label-result',
                    entryId: 'entry',
                    label: `${url.searchParams.get('query')} label`,
                    sessionModified: '2026-10-20T00:00:00Z',
                    nodeTimestamp: '2026-10-20T00:00:00Z',
                    active: true,
                  },
                ],
              },
            },
      )
    }
    return route.fulfill({ json: {} })
  })
  const url = `http://127.0.0.1:${server.httpServer.address().port}/__shell`
  await page.goto(url)
  const handle = page.getByTestId('sidebar-resize-handle')
  await handle.waitFor()
  assert.equal(await handle.getAttribute('aria-valuenow'), '340')
  await handle.focus()
  await handle.press('ArrowRight')
  assert.equal(await page.getByTestId('preferred').textContent(), '350')
  await handle.press('End')
  assert.equal(await page.getByTestId('preferred').textContent(), '520')
  await page.reload()
  await handle.waitFor()
  assert.equal(await handle.getAttribute('aria-valuenow'), '520')
  const box = await handle.boundingBox()
  await page.mouse.move(box.x + box.width / 2, box.y + 100)
  await page.mouse.down()
  await page.mouse.move(box.x - 80, box.y + 100)
  assert.equal(
    await page.getByTestId('preferred').textContent(),
    '520',
    'drag preview does not persist every pointer event',
  )
  await page.mouse.up()
  assert.ok(Number(await page.getByTestId('preferred').textContent()) < 450)
  const persistedWidth = await page.getByTestId('preferred').textContent()
  for (const viewportWidth of [650, 651, 767]) {
    await page.setViewportSize({ width: viewportWidth, height: 800 })
    await handle.waitFor({ state: 'hidden' })
    const trigger = page.getByTestId('sidebar-toggle')
    await trigger.click()
    const sidebar = page.getByRole('dialog')
    await sidebar.waitFor()
    assert.equal(await sidebar.getAttribute('data-mobile'), 'true')
    await page.keyboard.press('Escape')
    await sidebar.waitFor({ state: 'hidden' })
    assert.equal(
      await trigger.evaluate((element) => element === document.activeElement),
      true,
      `sidebar at ${viewportWidth}px returns focus to its trigger`,
    )
  }
  await page.setViewportSize({ width: 768, height: 800 })
  await handle.waitFor()
  assert.equal(await page.getByRole('dialog').count(), 0)
  assert.equal(await page.getByTestId('preferred').textContent(), persistedWidth)
  await page.setViewportSize({ width: 1200, height: 800 })
  await page.getByTestId('launch').click()
  const input = page.getByRole('combobox')
  await input.waitFor()
  const dialog = page.getByRole('dialog')
  await dialog.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished)),
  )
  assert.ok(Math.abs((await dialog.boundingBox()).width - 520) < 1)
  await input.dispatchEvent('compositionstart')
  await input.dispatchEvent('keydown', { key: 'Enter', isComposing: true })
  await input.dispatchEvent('keydown', { key: 'Escape', isComposing: true })
  assert.equal(
    await dialog.count(),
    1,
    'IME confirmation and cancellation must not activate or dismiss',
  )
  await input.dispatchEvent('compositionend')
  await input.press('Tab')
  const activeGroup = await input.evaluate((element) =>
    document
      .getElementById(element.getAttribute('aria-activedescendant'))
      .closest('[role=group]')
      .getAttribute('aria-label'),
  )
  assert.match(activeGroup, /页面|Pages/)
  failLabels = true
  await input.fill('Alpha')
  const retry = page
    .getByRole('option')
    .filter({ hasText: /无法搜索会话标记|Could not search conversation labels/ })
  await retry.waitFor()
  assert.equal(await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).count(), 1)
  failLabels = false
  await retry.click()
  await page.getByRole('option').filter({ hasText: 'Alpha label' }).waitFor()
  assert.ok(labelRequests >= 2)
  await input.fill('Failure')
  await page.getByRole('option').filter({ hasText: 'Failure conversation' }).click()
  await page.getByRole('alert').waitFor()
  assert.equal(await dialog.count(), 1, 'failed navigation retains the result and retry path')
  await input.fill('Alpha')
  await page.mouse.move(10, 10)
  await page.getByRole('option').filter({ hasText: 'Alpha label' }).waitFor()
  await input.fill('Alpha ')
  await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).hover()
  const before = await page.getByRole('option').allTextContents()
  holdLabels = true
  await page.evaluate(() => {
    window.dispatchEvent(new Event('focus'))
  })
  // 改查询后重新请求，指针进入已有会话结果时冻结快照。
  await input.fill('Alph')
  await page.getByRole('option').filter({ hasText: 'Alpha conversation' }).hover()
  await page.waitForFunction(
    () => document.querySelector('[role=listbox]')?.getAttribute('aria-busy') === 'true',
  )
  let requestTimer
  try {
    await Promise.race([
      labelRequestStarted,
      new Promise((_, reject) => {
        requestTimer = setTimeout(
          () => reject(new Error('Pending label request was not observed')),
          6000,
        )
      }),
    ])
  } finally {
    clearTimeout(requestTimer)
  }
  releaseLabels()
  holdLabels = false
  await page.waitForFunction(
    () => document.querySelector('[role=listbox]')?.getAttribute('aria-busy') === 'false',
  )
  assert.equal(
    await page.getByRole('option').filter({ hasText: 'Alph label' }).count(),
    0,
    'late results cannot move the hovered target',
  )
  await page.mouse.move(10, 10)
  await page.getByRole('option').filter({ hasText: 'Alph label' }).waitFor()
  assert.ok(before.length)
  const desktopMetrics = await dialog.evaluate((element) => {
    const style = getComputedStyle(element)
    const rect = element.getBoundingClientRect()
    return {
      width: rect.width,
      height: rect.height,
      radius: style.borderRadius,
      fontFamily: style.fontFamily,
      fontSize: style.fontSize,
      rootFontSize: getComputedStyle(document.documentElement).fontSize,
      devicePixelRatio,
    }
  })
  await page.screenshot({ path: join(output, 'search-light.png') })
  await page.evaluate(() => {
    document.documentElement.dataset.theme = 'dark'
    document.documentElement.classList.add('dark')
  })
  await page.screenshot({ path: join(output, 'search-dark.png') })
  await page.setViewportSize({ width: 390, height: 420 })
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await dialog.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished)),
  )
  const narrow = await dialog.boundingBox()

  assert.ok(narrow.x >= 0 && narrow.x + narrow.width <= 390, JSON.stringify(narrow))
  await page.screenshot({ path: join(output, 'search-narrow.png') })
  assert.deepEqual(errors, [])
  await writeFile(
    join(output, 'metrics.json'),
    JSON.stringify(
      {
        browser: await browser.version(),
        locale: 'zh-CN',
        timezone: 'Asia/Shanghai',
        fixtureTime: '2026-10-20T09:00:00Z',
        desktop: { viewport: { width: 1200, height: 800 }, ...desktopMetrics },
        narrow: { viewport: { width: 390, height: 420 }, dialog: narrow },
        referenceScreenshotCompared: false,
      },
      null,
      2,
    ),
  )
  console.log(
    `PASS replica shell: resize/restore, 650/651/767/768 breakpoint/focus, IME, grouped Tab, errors/retry, pointer stability; snapshots ${output}`,
  )
} finally {
  await browser?.close()
  await server.close()
}
