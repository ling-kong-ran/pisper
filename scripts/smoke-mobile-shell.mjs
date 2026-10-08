// 移动壳回归：真实 React 组件、隔离接口/桥接和本机浏览器，不启动模型或读取用户配置。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { readFile, readdir } from 'node:fs/promises'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const mocks = {
  '@/components/common/MarkdownMessage':
    'export default function MarkdownMessage() { return null }',
  '@/app/i18n/use-i18n': `export function useI18n() { return {t: key => key, language: 'en-US'} }`,
  '@/features/chat/api/chat-api': `
    const data = () => ({files:[{path:'sample.txt',pending:true,approved:false,reverted:false,canRevert:true,added:1,removed:0}],summary:{files:window.evidence.revision,pending:1,added:1,removed:0}})
    export const chatApi = {
      getSessionFileChanges:async () => { window.evidence.reads++; return data() },
      approveSessionFileChanges:async () => { window.evidence.approvals++; return data() },
      revertSessionFileChanges:async () => { window.evidence.reverts++; return data() },
    }`,
  '@/features/chat/model/session-change-summary-api': `export async function invalidateSessionChangeSummary() {}`,
  '@/lib/http/api': `export async function apiJson(url, options) {
    if (url.startsWith('/api/assets?')) return {assets:[]}
    if (url !== '/api/plugins') throw new Error('Unexpected fixture API '+url)
    if (options?.method === 'PUT') {
      window.evidence.pluginWrites++
      const patch = JSON.parse(options.body)
      window.pluginData = {...window.pluginData,...patch,plugins:window.pluginData.plugins.map(plugin => ({...plugin,capabilities:plugin.capabilities.map(tool => ({...tool,enabled:patch.enabledTools.includes(tool.name)}))}))}
    }
    return structuredClone(window.pluginData)
  }`,
}
const output = await build({
  entryPoints: [join(root, 'scripts/fixtures/mobile-shell.jsx')],
  bundle: true,
  write: false,
  loader: { '.css': 'empty' },
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
          mocks[args.path] ? { path: args.path, namespace: 'mock' } : null,
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
const assets = await readdir(join(root, 'dist/assets'))
const css = (
  await Promise.all(
    assets
      .filter((file) => file.endsWith('.css'))
      .map((file) => readFile(join(root, 'dist/assets', file), 'utf8')),
  )
).join('\n')
const server = createServer((request, response) => {
  const url = request.url
  response.setHeader(
    'Content-Type',
    url === '/bundle.js' ? 'text/javascript' : url === '/style.css' ? 'text/css' : 'text/html',
  )
  response.end(
    url === '/bundle.js'
      ? js
      : url === '/style.css'
        ? css
        : '<!doctype html><html><head><link rel="stylesheet" href="/style.css"></head><body style="margin:0"><div id="root"></div><script type="module" src="/bundle.js"></script></body></html>',
  )
})
let browser
try {
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
  assert(
    existsSync(executablePath),
    'Install Chromium with npx playwright-core install chromium, or set PISPER_SMOKE_BROWSER.',
  )
  browser = await chromium.launch({ headless: true, executablePath })
  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, hasTouch: true })
  page.setDefaultTimeout(10000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`http://127.0.0.1:${server.address().port}`)
  await page.waitForFunction(() => window.workspaceEntries)
  assert.deepEqual(
    await page.evaluate(() =>
      window.workspaceEntries.normalizeWorkspaceEntries({
        entries: [
          { name: 'z.txt', type: 'file' },
          { name: 'folder', type: 'directory' },
        ],
      }),
    ),
    [
      { name: 'folder', kind: 'directory' },
      { name: 'z.txt', kind: 'file' },
    ],
  )
  assert.deepEqual(
    await page.evaluate(() =>
      window.workspaceEntries.normalizeWorkspaceEntries({
        directories: ['folder'],
        files: ['z.txt'],
      }),
    ),
    [
      { name: 'folder', kind: 'directory' },
      { name: 'z.txt', kind: 'file' },
    ],
  )
  assert.deepEqual(
    await page.evaluate(() => window.workspaceEntries.normalizeWorkspaceEntries({ entries: [] })),
    [],
  )
  assert.equal(
    await page.evaluate(() => {
      try {
        window.workspaceEntries.normalizeWorkspaceEntries({})
        return false
      } catch (error) {
        return error.kind === 'protocol'
      }
    }),
    true,
  )
  await page.getByRole('button', { name: 'newTerminal', exact: true }).click()
  await page.waitForFunction(() => window.evidence.created === 1)
  await page.evaluate(() => window.controls.setPane('chat'))
  await page.waitForFunction(
    () => document.querySelector('section[aria-label="navigation:mobileShell.context"]').inert,
  )
  await page.evaluate(() => window.controls.setPane('context'))
  await page.locator('.terminal-tab').waitFor()
  await page.evaluate(() => window.controls.setTab('extensions'))
  const toggle = page.getByRole('switch').first()
  await toggle.waitFor()
  await toggle.click()
  await page.getByText('plugins:pluginsPage.unsaved', { exact: true }).waitFor()
  await page.evaluate(() => window.controls.setTab('terminal'))
  await page.locator('.terminal-tab').waitFor()
  assert.equal(
    await page.evaluate(() => window.evidence.closeAll),
    0,
    'Pane/tab changes must keep the running terminal',
  )
  await page.evaluate(() => window.controls.setTab('extensions'))
  assert.equal(
    await toggle.getAttribute('aria-checked'),
    'false',
    'Switching tabs preserves unsaved plugin edits',
  )
  await page.getByRole('button', { name: 'plugins:pluginsPage.savePolicy', exact: true }).click()
  await page.waitForFunction(() => window.evidence.pluginWrites === 1)
  await page.getByText('plugins:pluginsPage.unsaved', { exact: true }).waitFor({ state: 'hidden' })
  assert.deepEqual(await page.evaluate(() => window.pluginData.enabledTools), [])

  await page.evaluate(() => window.controls.setTab('assets'))
  await page.getByRole('button', { name: 'assets:assetsPage.addLink', exact: true }).first().click()
  await page.getByRole('heading', { name: 'assets:assetsPage.addLinkAsset', exact: true }).waitFor()
  const backdrop = await page.locator('.modal-backdrop').boundingBox()
  assert(
    backdrop && Math.abs(backdrop.x) < 1 && Math.abs(backdrop.width - 390) < 1,
    'Embedded asset modal must cover the viewport rather than the translated track',
  )
  await page.getByRole('button', { name: 'assets:assetsPage.closeDialog', exact: true }).click()
  await page.evaluate(() => window.controls.setTab('changes'))
  const approve = page.getByRole('button', {
    name: 'chat:focusSession.fileChangesApproveAll',
    exact: true,
  })
  await page.waitForFunction(() => window.evidence.reads > 0)
  await page.waitForFunction(
    () =>
      ![...document.querySelectorAll('button')].find(
        (node) => node.textContent === 'chat:focusSession.fileChangesApproveAll',
      ).disabled,
  )
  await page.evaluate(() => window.controls.setStreaming(true))
  await page.waitForFunction(
    () =>
      [...document.querySelectorAll('button')].find(
        (node) => node.textContent === 'chat:focusSession.fileChangesApproveAll',
      ).disabled,
  )
  assert.equal(await approve.isDisabled(), true)
  const reads = await page.evaluate(() => window.evidence.reads)
  await page.evaluate(() => {
    window.evidence.revision++
    window.controls.setStreaming(false)
  })
  await page.waitForFunction((previous) => window.evidence.reads > previous, reads)
  await page.waitForFunction(
    () =>
      ![...document.querySelectorAll('button')].find(
        (node) => node.textContent === 'chat:focusSession.fileChangesApproveAll',
      ).disabled,
  )
  await page
    .getByRole('button', { name: 'chat:focusSession.fileChangesRevertAll', exact: true })
    .click()
  await page.waitForFunction(() => window.confirmPending)
  const readsBeforeConfirmRun = await page.evaluate(() => window.evidence.reads)
  await page.evaluate(() => window.controls.setStreaming(true))
  await page.waitForFunction((previous) => window.evidence.reads > previous, readsBeforeConfirmRun)
  await page.waitForFunction(
    () =>
      [...document.querySelectorAll('button')].find(
        (node) => node.textContent === 'chat:focusSession.fileChangesApproveAll',
      ).disabled,
  )
  await page.evaluate(async () => {
    window.resolveConfirm(true)
    await new Promise(requestAnimationFrame)
  })
  await page.evaluate(() => window.controls.setStreaming(false))
  await page.waitForFunction(
    () =>
      ![...document.querySelectorAll('button')].find(
        (node) => node.textContent === 'chat:focusSession.fileChangesApproveAll',
      ).disabled,
  )
  assert.equal(
    await page.evaluate(() => window.evidence.reverts),
    0,
    'A run starting during confirmation must cancel the stale revert',
  )
  await page.evaluate(() => window.controls.setStreaming(null))
  await page
    .getByRole('button', { name: 'navigation:mobileShell.changesOpenChat', exact: true })
    .waitFor()
  assert.equal(await approve.count(), 0, 'No stale writes after the chat owner leaves')

  await page.setViewportSize({ width: 1024, height: 768 })
  await page.evaluate(() => window.controls.setMode('pad'))
  await page.waitForFunction(() => document.querySelector('[data-mobile-shell="pad"]'))
  const widths = await page
    .locator('[data-mobile-shell] > div > section')
    .evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().width))
  assert.equal(widths.length, 3)
  assert(widths.every((width) => width > 0))
  assert.equal(Math.round(widths.reduce((sum, width) => sum + width, 0)), 1024)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.evaluate(() => {
    window.controls.setMode('phone')
    window.controls.setPane('chat')
  })
  await page.waitForFunction(() => document.querySelector('[data-mobile-shell="phone"]'))
  const cdp = await page.context().newCDPSession(page)
  for (const [type, points] of [
    ['touchStart', [{ x: 270, y: 300 }]],
    ['touchMove', [{ x: 150, y: 300 }]],
    ['touchMove', [{ x: 80, y: 300 }]],
    ['touchEnd', []],
  ])
    await cdp.send('Input.dispatchTouchEvent', { type, touchPoints: points })
  await page.waitForFunction(
    () => !document.querySelector('section[aria-label="navigation:mobileShell.context"]').inert,
  )
  assert.equal(await page.evaluate(() => window.evidence.closeAll), 0)
  await page.evaluate(() => window.controls.setMounted(false))
  await page.waitForFunction(() => window.evidence.closeAll === 1)
  assert.deepEqual(errors, [])
  console.log(
    'PASS mobile shell: terminal lifetime, independent save, run safety, refresh, layout and touch navigation',
  )
} finally {
  await browser?.close()
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
}
