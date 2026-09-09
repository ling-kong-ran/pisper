import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const baseline = process.argv.includes('--baseline')
const fixturePath = String.raw`C:\Users\Administrator\generated\visuals\chibi_character_replacement_corrected.gif`

// 同一浏览器分别渲染发布源码和工作区源码，实际点击 React 按钮；不直接调用 revealPath 绕过 UI。
const snapshotPlugin = {
  name: 'released-reveal-snapshot',
  setup(builder) {
    if (!baseline) return
    builder.onLoad({ filter: /(?:MarkdownMessage\.tsx|local-file-links\.ts)$/ }, (args) => {
      const path = args.path.replaceAll('\\', '/').slice(root.replaceAll('\\', '/').length + 1)
      return {
        contents: execFileSync('git', ['show', `v0.5.57:${path}`], { cwd: root, encoding: 'utf8' }),
        loader: path.endsWith('.tsx') ? 'tsx' : 'ts',
        resolveDir: dirname(args.path),
      }
    })
  },
}
const fixture = await build({
  stdin: {
    contents: `
      import React from 'react'
      import {createRoot} from 'react-dom/client'
      import MarkdownMessage from './src/components/MarkdownMessage.tsx'
      import {LOCAL_REVEAL_NOTICE_EVENT} from './src/app/route-context.ts'
      const root = createRoot(document.getElementById('root'))
      window.notices = []
      window.calls = []
      window.addEventListener(LOCAL_REVEAL_NOTICE_EVENT, event => window.notices.push(event.detail))
      window.setBridge = mode => {
        if (mode === 'missing') { delete window.pisperDesktop; return }
        window.pisperDesktop = {revealPath(path) {
          window.calls.push(path)
          if (mode === 'sync-error') throw new Error('Tauri IPC is unavailable.')
          if (mode === 'denied') return Promise.reject('desktop_reveal_path not allowed on window main')
          if (mode === 'pending') return new Promise(() => {})
          return Promise.resolve(mode !== 'false')
        }}
      }
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
  plugins: [snapshotPlugin],
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
  const pageErrors = []
  page.on('pageerror', (error) => pageErrors.push(error.message))
  const url = `http://127.0.0.1:${server.address().port}`
  async function mount(mode) {
    await page.goto(url)
    await page.waitForFunction(() => typeof window.mount === 'function')
    await page.evaluate(
      ({ mode, path }) => {
        window.setBridge(mode)
        window.mount('[GIF](' + path + ')')
      },
      { mode, path: fixturePath },
    )
    await page.locator('[data-local-path]').waitFor()
  }

  await mount('missing')
  const initialTag = await page.locator('[data-local-path]').evaluate((element) => element.tagName)
  assert.equal(initialTag, baseline ? 'SPAN' : 'BUTTON')
  await page.locator('[data-local-path]').click()
  if (baseline) {
    assert.equal(await page.evaluate(() => window.notices.length), 0)
    // 旧组件在渲染时捕获空桥接：即便桥接稍后出现，已显示的 span 仍然不能点击。
    await page.evaluate(() => window.setBridge('success'))
    await page.locator('[data-local-path]').click()
    assert.equal(await page.evaluate(() => window.calls.length), 0)
    console.log('v0.5.57 reproduced: missing/late bridge -> SPAN, no request, no notice')
  } else {
    await page.getByRole('alert').waitFor()
    assert.match(await page.getByRole('alert').innerText(), /local-reveal:unavailable/)
    await page.getByRole('button', { name: '复制路径' }).click()
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), fixturePath)
    await page.evaluate(() => window.setBridge('success'))
    await page.locator('[data-local-path]').click()
    await page.waitForFunction(() =>
      window.notices.some((notice) => notice.message.includes('已请求')),
    )
    assert.deepEqual(await page.evaluate(() => window.calls), [fixturePath])
    assert.equal(await page.getByRole('alert').count(), 0)
    console.log('PASS missing bridge feedback, copy path, and late bridge recovery')
  }

  await mount('success')
  await page.locator('[data-local-path]').click()
  await page.waitForFunction(() => window.notices.length === 1)
  assert.deepEqual(await page.evaluate(() => window.calls), [fixturePath])
  assert.equal(await page.evaluate(() => window.notices[0].tone), 'info')
  console.log('PASS normal Markdown click reaches the bridge once with the exact Windows path')

  if (!baseline) {
    for (const [mode, expected] of [
      ['sync-error', 'Tauri IPC is unavailable.'],
      ['denied', 'desktop_reveal_path not allowed'],
      ['false', 'local-reveal:failed'],
      ['pending', 'local-reveal:timeout'],
    ]) {
      await mount(mode)
      await page.locator('[data-local-path]').click()
      await page.getByRole('alert').waitFor({ timeout: 12_000 })
      assert.ok((await page.getByRole('alert').innerText()).includes(expected))
      assert.equal(await page.evaluate(() => window.notices[0].tone), 'error')
      assert.equal(await page.locator('[data-local-path]').isDisabled(), false)
      assert.equal(await page.evaluate(() => window.calls.length), 1)
      console.log(`PASS ${mode}: visible error, no automatic retry`)
    }
  }
  assert.deepEqual(pageErrors, [])
} finally {
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
