import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const fixturePath = String.raw`C:\Users\Administrator\generated\visuals\chibi_character_replacement_corrected.gif`
const fixture = await build({
  stdin: {
    contents: `
      import React from 'react'
      import { createRoot } from 'react-dom/client'
      import MarkdownMessage from './src/components/MarkdownMessage.tsx'
      const root = createRoot(document.getElementById('root'))
      window.notices = []
      window.addEventListener('pisper:local-reveal-notice', event => window.notices.push(event.detail))
      window.mount = source => root.render(React.createElement(MarkdownMessage, null, source))
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
const server = createServer((request, response) => {
  if (request.url === '/fixture.js') {
    response.writeHead(200, { 'Content-Type': 'text/javascript' })
    response.end(fixture.outputFiles[0].text)
  } else {
    response.writeHead(200, { 'Content-Type': 'text/html' })
    response.end(
      '<!doctype html><div id="root"></div><script type="module" src="/fixture.js"></script>',
    )
  }
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath =
    process.env.PISPER_SMOKE_BROWSER ||
    [
      chromium.executablePath(),
      'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
      'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
    ].find(existsSync)
  assert(executablePath, 'Set PISPER_SMOKE_BROWSER to an installed Chromium/Edge executable.')
  browser = await chromium.launch({ executablePath, headless: true })
  const context = await browser.newContext({ permissions: ['clipboard-read', 'clipboard-write'] })
  const page = await context.newPage()
  page.on('pageerror', (error) => console.error(`浏览器页面错误：${error.message}`))
  page.on('console', (message) => {
    if (message.type() === 'error') console.error(`浏览器控制台错误：${message.text()}`)
  })
  const calls = []
  let mode = 'success'
  await page.route('**/api/desktop/reveal-path', async (route) => {
    const requestBody = route.request().postDataJSON()
    calls.push(requestBody.path)
    if (mode === 'denied') {
      await route.fulfill({
        status: 403,
        contentType: 'application/json',
        body: JSON.stringify({ error: '远程客户端不能打开宿主文件管理器。' }),
      })
      return
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ revealed: mode !== 'false', path: requestBody.path }),
    })
  })
  const url = `http://127.0.0.1:${server.address().port}`
  async function mount(nextMode) {
    mode = nextMode
    calls.length = 0
    await page.goto(url)
    await page.waitForFunction(() => typeof window.mount === 'function')
    await page.evaluate((path) => window.mount('[GIF](' + path + ')'), fixturePath)
    await page.locator('[data-local-path]').waitFor()
  }

  await mount('success')
  assert.equal(
    await page.locator('[data-local-path]').evaluate((element) => element.tagName),
    'BUTTON',
  )
  await page.locator('[data-local-path]').click()
  await page.waitForFunction(() =>
    window.notices.some((notice) => notice.message.includes('已请求')),
  )
  assert.deepEqual(calls, [fixturePath])
  console.log('PASS Runtime endpoint click opens with the exact Windows path')

  await mount('false')
  await page.locator('[data-local-path]').click()
  await page.getByRole('alert').waitFor()
  assert.match(await page.getByRole('alert').innerText(), /无法在文件管理器中显示/)
  assert.deepEqual(calls, [fixturePath])
  console.log('PASS false Runtime result produces visible feedback without retry')

  await mount('denied')
  await page.locator('[data-local-path]').click()
  await page.getByRole('alert').waitFor()
  assert.match(await page.getByRole('alert').innerText(), /无法在文件管理器中显示/)
  assert.deepEqual(calls, [fixturePath])
  console.log('PASS Runtime HTTP errors produce visible feedback without retry')
} finally {
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
