import assert from 'node:assert/strict'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import { chromium } from 'playwright-core'

const output = await mkdtemp(join(tmpdir(), 'pisper-replica-chat-'))
const errors = []
const server = await createServer({
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
  plugins: [
    {
      name: 'replica-chat-fixture',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          const url = new URL(req.url, 'http://fixture')
          if (url.pathname === '/__chat') {
            res.setHeader('Content-Type', 'text/html')
            res.end(
              await server.transformIndexHtml(
                '/__chat',
                '<!doctype html><html lang="en-US"><head><meta name="viewport" content="width=device-width,initial-scale=1"></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/replica-chat.tsx"></script></body></html>',
              ),
            )
          } else if (url.pathname.startsWith('/api/')) {
            res.setHeader('Content-Type', 'application/json')
            res.end(
              JSON.stringify(
                url.pathname.endsWith('/workflow-runs')
                  ? { runs: [] }
                  : { assets: [], sessions: [], files: [], plugins: [], enabledTools: [] },
              ),
            )
          } else next()
        })
      },
    },
  ],
})
let browser
try {
  await server.listen()
  browser = await chromium.launch({ headless: true })
  const page = await browser.newPage({ viewport: { width: 1280, height: 1000 }, locale: 'en-US' })
  page.setDefaultTimeout(15000)
  await page.emulateMedia({ reducedMotion: 'reduce' })
  page.on('pageerror', (error) => errors.push(error.message))
  await page.addInitScript(() => {
    window.clipboardEvidence = { count: 0, fail: false }
    Object.defineProperty(navigator, 'clipboard', {
      value: {
        writeText: () => {
          window.clipboardEvidence.count++
          return window.clipboardEvidence.fail
            ? Promise.reject(new Error('denied'))
            : Promise.resolve()
        },
      },
    })
  })
  await page.goto(`http://127.0.0.1:${server.httpServer.address().port}/__chat`)
  const input = page.locator('.focus-composer textarea')
  await input.waitFor({ timeout: 45000 })
  await page.locator('[data-pisper-message-id="agent"] h1').waitFor()
  const metrics = await page.locator('.focus-composer').evaluate((element) => {
    const input = element.querySelector('textarea')
    return {
      width: element.getBoundingClientRect().width,
      radius: getComputedStyle(element).borderRadius,
      inputLineHeight: getComputedStyle(input).lineHeight,
      inputMinHeight: getComputedStyle(input).minHeight,
    }
  })
  assert.equal(metrics.width, 768)
  assert.equal(metrics.radius, '22px')
  assert.equal(metrics.inputLineHeight, '20px')
  assert.equal(metrics.inputMinHeight, '44px')
  const typography = await page
    .locator('[data-pisper-message-id="agent"] .markdown-body')
    .evaluate((element) => {
      const style = getComputedStyle(element)
      return {
        size: style.fontSize,
        lineHeight: style.lineHeight,
        weight: style.fontWeight,
        headings: ['h1', 'h2', 'h3'].map((tag) => {
          const heading = getComputedStyle(element.querySelector(tag))
          return [heading.fontSize, heading.lineHeight]
        }),
      }
    })
  assert.deepEqual(typography, {
    size: '16px',
    lineHeight: '26px',
    weight: '400',
    headings: [
      ['24px', '32px'],
      ['20px', '28px'],
      ['18px', '28px'],
    ],
  })
  assert.equal(
    await page
      .locator('[data-pisper-message-id="user"] .markdown-content>p')
      .first()
      .evaluate((element) => getComputedStyle(element).marginBlock),
    '0px',
  )
  await input.fill('failed draft')
  await page.locator('.send-button').click()
  assert.equal(await input.inputValue(), 'failed draft')
  await page.evaluate(() => window.settleSend(false))
  await page.waitForFunction(() => !document.querySelector('.send-button').disabled)
  assert.equal(await input.inputValue(), 'failed draft')
  await input.fill('accepted draft')
  await page.locator('.send-button').click()
  await page.evaluate(() => window.settleSend(true))
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await input.fill('running guidance')
  await page.locator('.send-button').click()
  await page.waitForFunction(() => window.replicaChat.evidence.queues === 1)
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await page.locator('.send-button').click()
  await page.waitForFunction(() => window.replicaChat.evidence.aborts === 1)
  assert.equal(await page.locator('.send-button').getAttribute('aria-busy'), 'true')
  await page.evaluate(() => window.settleAbort())
  const copy = page.locator('[data-pisper-message-id="agent"] .message-actions button').first()
  await copy.click()
  await copy.dispatchEvent('click')
  assert.equal(await page.evaluate(() => window.clipboardEvidence.count), 1)
  assert.equal(await copy.getAttribute('aria-disabled'), 'true')
  assert.equal(
    await copy.evaluate((element) => element.disabled),
    false,
    'Copied feedback prevents activation without removing the message action from keyboard focus',
  )
  await copy.focus()
  assert.equal(await copy.evaluate((element) => element === document.activeElement), true)
  await input.fill('session a')
  await page.evaluate(() => window.replicaChat.switchSession())
  await page.waitForFunction(() => document.querySelector('.focus-composer textarea').value === '')
  await input.fill('session b')
  await page.evaluate(() => window.replicaChat.switchSession())
  await page.waitForFunction(
    () => document.querySelector('.focus-composer textarea').value === 'session a',
  )
  await page.evaluate(() => {
    const read = File.prototype.text
    File.prototype.text = function () {
      if (this.name !== 'slow.txt') return read.call(this)
      return new Promise((resolve) => {
        window.releaseFile = () => resolve('complete content')
      })
    }
    const transfer = new DataTransfer()
    transfer.items.add(new File(['complete content'], 'slow.txt', { type: 'text/plain' }))
    document
      .querySelector('.focus-composer textarea')
      .dispatchEvent(
        new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }),
      )
  })
  await page.locator('.attachment-tray [aria-busy=true]').waitFor()
  assert.equal(await page.locator('.send-button').getAttribute('aria-busy'), 'true')
  await page.locator('.attachment-tray [aria-busy=true] button').click()
  await page.evaluate(() => window.releaseFile())
  assert.equal(await page.locator('.attachment-chip').count(), 0)
  await page.screenshot({ path: join(output, 'desktop.png'), fullPage: true })
  const footerWidths = []
  for (const width of [299, 300, 439, 440, 519, 520]) {
    await page.locator('.focus-composer-footer').evaluate((element, width) => {
      element.style.width = `${width}px`
    }, width)
    footerWidths.push(
      await page.locator('.focus-composer-footer').evaluate((element) => ({
        width: element.getBoundingClientRect().width,
        overflow: element.scrollWidth > element.clientWidth,
      })),
    )
  }
  assert.ok(
    footerWidths.every((row) => !row.overflow),
    JSON.stringify(footerWidths),
  )
  await page.locator('.focus-composer-footer').evaluate((element) => {
    element.style.width = ''
  })
  await page.setViewportSize({ width: 390, height: 844 })
  await page.screenshot({ path: join(output, 'mobile.png'), fullPage: true })
  assert.ok(
    await page
      .locator('.focus-composer')
      .evaluate((element) => element.scrollWidth <= element.clientWidth),
  )
  await page.setViewportSize({ width: 1280, height: 1000 })
  await page.evaluate(() => window.replicaChat.history())
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-pisper-transcript-size]')
        ?.getAttribute('data-pisper-transcript-size') === '200',
  )
  await page.waitForFunction(() => !!document.querySelector('[data-pisper-virtual-item="m499"]'))
  const transcript = page.locator('.transcript')
  await transcript.evaluate((element) => {
    element.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, bubbles: true }))
    element.scrollTop = 140
  })
  await page.waitForFunction(() => !!document.querySelector('[data-pisper-virtual-item="m300"]'))
  const anchor = page.locator('[data-pisper-virtual-item="m300"]')
  const before = await anchor.evaluate((element) => element.getBoundingClientRect().top)
  await page.locator('.history-page-loader button').dispatchEvent('click')
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-pisper-transcript-size]')
        ?.getAttribute('data-pisper-transcript-size') === '200' &&
      !!document.querySelector('[data-pisper-virtual-item="m300"]') &&
      !![...document.querySelectorAll('button')].find((button) =>
        /Return to latest|返回最新/.test(button.textContent),
      ),
  )
  await page.waitForFunction((before) => {
    const row = document.querySelector('[data-pisper-virtual-item="m300"]')
    return row && Math.abs(row.getBoundingClientRect().top - before) < 4
  }, before)
  const after = await anchor.evaluate((element) => element.getBoundingClientRect().top)
  assert.ok(Math.abs(after - before) < 4, `Reading anchor moved by ${after - before}px`)
  await page.getByRole('button', { name: /Return to latest|返回最新/ }).click()
  await page.waitForFunction(() => !!document.querySelector('[data-pisper-virtual-item="m499"]'))
  assert.deepEqual(errors, [])
  await writeFile(
    join(output, 'metrics.json'),
    JSON.stringify({ metrics, typography, footerWidths }, null, 2),
  )
  console.log(`Replica chat smoke passed: ${output}`)
} finally {
  await browser?.close()
  await server.close()
}
