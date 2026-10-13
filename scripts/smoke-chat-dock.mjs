// 桌面分屏回归：真实 React/Dockview 和本机 Chromium，隔离模型、用户数据与 Runtime。
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
  '@/lib/ui/shortcuts': 'export function useShortcutLabel() { return "Ctrl+N" }',
  '@/features/chat/hooks/use-session-catalog': 'export const FOCUS_MESSAGE_PAGE_SIZE = 40',
  '@/lib/storage/page-state-storage': 'export const pageStateStorage = localStorage',
  '@/features/chat/components/WebPreviewDockPanel':
    'export function WebPreviewDockPanel() { return null }',
  '@/features/chat/components/runtime/FocusSession': `
    export function FocusSession({session,canClosePanel,onClosePanel}) {
      return <section data-session-id={session.id} style={{height:'100%',padding:12,boxSizing:'border-box',background:'#fff'}}>
        <h2>{session.name}</h2>
        <textarea aria-label={'Message '+session.name} defaultValue={'Draft '+session.name} />
        {canClosePanel && <button onClick={onClosePanel}>Close {session.name}</button>}
      </section>
    }`,
  '@/lib/http/api': `
    export async function apiJson(url, options) {
      if (url !== '/api/settings/chat-dock-layout') throw new Error('Unexpected fixture API: '+url)
      if (options?.method === 'PUT') window.evidence.writeAttempts++
      const response = await fetch(url, options)
      if (!response.ok) throw new Error('Fixture HTTP '+response.status)
      const value = await response.json()
      window.evidence[options?.method === 'PUT' ? 'writes' : 'reads']++
      return value
    }`,
}
const output = await build({
  entryPoints: [join(root, 'scripts/fixtures/chat-dock.jsx')],
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
          mocks[args.path] ? { path: args.path, namespace: 'mock' } : null,
        )
        builder.onLoad({ filter: /.*/, namespace: 'mock' }, (args) => ({
          contents: mocks[args.path],
          loader: 'jsx',
          resolveDir: root,
        }))
      },
    },
  ],
})
const js = output.outputFiles.find((file) => file.path.endsWith('.js')).text
const css = output.outputFiles.find((file) => file.path.endsWith('.css'))?.text || ''
let savedLayout = null
let readGate = null
let failRead = false
const server = createServer(async (request, response) => {
  if (request.url === '/api/settings/chat-dock-layout') {
    response.setHeader('Content-Type', 'application/json')
    if (request.method === 'PUT') {
      let body = ''
      for await (const chunk of request) body += chunk
      savedLayout = JSON.parse(body)
      response.end(JSON.stringify(savedLayout))
    } else {
      await readGate?.promise
      response.statusCode = failRead ? 503 : 200
      response.end(JSON.stringify(savedLayout))
    }
    return
  }
  response.setHeader(
    'Content-Type',
    request.url === '/bundle.js'
      ? 'text/javascript'
      : request.url === '/style.css'
        ? 'text/css'
        : 'text/html',
  )
  response.end(
    request.url === '/bundle.js'
      ? js
      : request.url === '/style.css'
        ? css
        : `<!doctype html><html><head><link rel="stylesheet" href="/style.css"><style>
    html,body,#root {height:100%;margin:0} #root{display:flex;flex-direction:column;font-family:system-ui}
    main>div{height:100%;min-height:0} .session-dock-panel{height:100%;width:100%} button,select{font:inherit}
  </style></head><body><div id="root"></div><script type="module" src="/bundle.js"></script></body></html>`,
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
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
  page.setDefaultTimeout(10000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const origin = `http://127.0.0.1:${server.address().port}`
  const visibleIds = () =>
    page
      .locator('[data-session-id]:visible')
      .evaluateAll((nodes) => nodes.map((node) => node.dataset.sessionId).sort())
  const expectIds = async (ids) => {
    await page.waitForFunction(
      (expected) => {
        const actual = [...document.querySelectorAll('[data-session-id]')]
          .filter(
            (node) =>
              getComputedStyle(node).visibility === 'visible' &&
              node.getBoundingClientRect().width > 0 &&
              node.getBoundingClientRect().height > 0,
          )
          .map((node) => node.dataset.sessionId)
          .sort()
        return JSON.stringify(actual) === JSON.stringify(expected)
      },
      [...ids].sort(),
    )
    assert.deepEqual(await visibleIds(), [...ids].sort())
  }
  const saved = async () => {
    const before = await page.evaluate(() => {
      const count = window.evidence.writes
      window.dispatchEvent(new Event('pagehide'))
      return count
    })
    // pagehide 会立即排队保存；以对应 HTTP 的完成为界，避免测试依赖保存延迟。
    await page.waitForFunction((count) => window.evidence.writes > count, before)
    return structuredClone(savedLayout)
  }
  const fresh = async (layout = null, query = '') => {
    await page.goto('about:blank')
    savedLayout = layout
    await page.goto(origin + '/' + query)
    await page.waitForFunction(() => Boolean(window.controls))
  }
  const open = async (name, disposition = 'open') => {
    await page.getByRole('combobox', { name: 'Open position' }).selectOption(disposition)
    await page.getByRole('button', { name: `Open ${name}`, exact: true }).click()
  }
  const checkDirection = async (direction) => {
    await expectIds(['alpha', 'beta'])
    const alpha = await page.locator('[data-session-id="alpha"]').boundingBox()
    const beta = await page.locator('[data-session-id="beta"]').boundingBox()
    assert.ok(
      alpha.width > 200 && beta.width > 200 && alpha.height > 100 && beta.height > 100,
      `Split panel dimensions: ${JSON.stringify({ alpha, beta })}`,
    )
    if (direction === 'left') assert.ok(beta.x + beta.width <= alpha.x + 2)
    if (direction === 'right') assert.ok(alpha.x + alpha.width <= beta.x + 2)
    if (direction === 'above') assert.ok(beta.y + beta.height <= alpha.y + 2)
    if (direction === 'below') assert.ok(alpha.y + alpha.height <= beta.y + 2)
  }
  const checkBounds = async () => {
    const overflow = await page.evaluate(() => {
      const container = document.querySelector('main').getBoundingClientRect()
      return [...document.querySelectorAll('[data-session-id]')]
        .filter((node) => {
          const rect = node.getBoundingClientRect()
          return (
            rect.left < container.left - 1 ||
            rect.right > container.right + 1 ||
            rect.top < container.top - 1 ||
            rect.bottom > container.bottom + 1
          )
        })
        .map((node) => node.dataset.sessionId)
    })
    assert.deepEqual(overflow, [], 'Each split stays fully inside the chat area')
  }

  await fresh(null, '?sidebar=1')
  await expectIds(['alpha'])
  const draft = 'Keep this draft while the context sidebar changes width'
  await page.getByRole('textbox', { name: 'Message Alpha', exact: true }).fill(draft)
  const resizeFrames = await page.evaluate(async () => {
    const main = document.querySelector('main')
    const snapshots = []
    for (const width of [553, 400, 760]) {
      snapshots.push(
        await new Promise((resolve) => {
          requestAnimationFrame(() => {
            main.style.width = `${width}px`
            // 在下一帧绘制前检查，不能等几百毫秒后掩盖拖动过程中内容仍用旧尺寸的问题。
            requestAnimationFrame(() => {
              const host = main.getBoundingClientRect()
              const panel = main.querySelector('[data-session-id="alpha"]').getBoundingClientRect()
              resolve({ host: host.width, panel: panel.width, left: panel.left - host.left })
            })
          })
        }),
      )
    }
    main.style.removeProperty('width')
    return snapshots
  })
  for (const frame of resizeFrames) {
    assert.ok(
      Math.abs(frame.host - frame.panel) <= 1 && Math.abs(frame.left) <= 1,
      `Session content must follow parent width before paint: ${JSON.stringify(frame)}`,
    )
  }
  assert.equal(
    await page.getByRole('textbox', { name: 'Message Alpha', exact: true }).inputValue(),
    draft,
  )
  console.log('PASS: parent-only resizing updates content before paint and retains the draft')

  for (const direction of ['left', 'right', 'above', 'below']) {
    await fresh()
    await expectIds(['alpha'])
    await open('Beta', direction)
    await checkDirection(direction)
    assert.equal(await page.locator('[data-active-id]').getAttribute('data-active-id'), 'beta')
  }
  console.log('PASS: four split directions show independent session panels')

  const persisted = await saved()
  await fresh(persisted)
  await checkDirection('below')
  assert.equal(await page.locator('[data-active-id]').getAttribute('data-active-id'), 'beta')
  await page.setViewportSize({ width: 390, height: 844 })
  await page.locator('main[data-mode="phone"]').waitFor()
  await expectIds(['beta'])
  await open('Gamma')
  await expectIds(['gamma'])
  await page.setViewportSize({ width: 1280, height: 900 })
  await page.locator('main[data-mode="desktop"]').waitFor()
  await page.waitForFunction(() => document.querySelectorAll('.dv-groupview').length >= 2)
  const restored = await saved()
  assert.ok(
    restored.layout.panels['session:alpha'] && restored.layout.panels['session:beta'],
    'Phone navigation preserves desktop panels',
  )
  assert.equal(await page.locator('[data-active-id]').getAttribute('data-active-id'), 'gamma')
  assert.ok(restored.layout.panels['session:gamma'], 'Mobile selection remains active on desktop')
  console.log('PASS: reload and desktop/phone/desktop retain the desktop split')

  await fresh()
  await expectIds(['alpha'])
  await open('Beta')
  await expectIds(['beta'])
  const betaTab = page.locator('.dv-tab').filter({ hasText: 'Beta' })
  await betaTab.click({ button: 'right' })
  await page.getByText('chat:chatPage.splitToRight', { exact: true }).click()
  await checkDirection('right')
  await page.evaluate(() => window.controls.startStream('beta'))
  await page.getByRole('button', { name: 'Close Beta', exact: true }).click()
  await expectIds(['alpha'])
  assert.ok(
    await page.evaluate(() =>
      window.evidence.releases.some(
        (entry) => entry.id === 'beta' && !entry.panelOpen && entry.localStreamOwned,
      ),
    ),
  )
  assert.deepEqual(await page.evaluate(() => window.evidence.stopped), [])
  assert.deepEqual(await page.evaluate(() => window.controls.inspect().localStreams), ['beta'])
  await open('Beta', 'right')
  await checkDirection('right')
  await page.getByRole('button', { name: 'Delete Beta', exact: true }).click()
  await expectIds(['alpha'])
  await page.getByRole('button', { name: 'Delete Alpha', exact: true }).click()
  await expectIds(['gamma'])
  console.log('PASS: tab split, close, local-stream ownership and deletion fallback')

  await fresh()
  await expectIds(['alpha'])
  await open('Beta')
  await expectIds(['beta'])
  const actions = page.getByRole('button', { name: 'chat:focusSession.chatActions', exact: true })
  await actions.focus()
  await actions.press('Enter')
  for (const direction of ['Left', 'Right', 'Top', 'Bottom']) {
    await page
      .getByRole('menuitem', { name: `chat:chatPage.splitTo${direction}`, exact: true })
      .waitFor({ state: 'visible' })
  }
  await page.keyboard.press('Escape')
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('aria-label') === 'chat:focusSession.chatActions',
  )
  assert.equal(await actions.evaluate((node) => node === document.activeElement), true)
  await actions.press('Enter')
  const splitRight = page.getByRole('menuitem', { name: 'chat:chatPage.splitToRight', exact: true })
  await splitRight.focus()
  await splitRight.press('Enter')
  await checkDirection('right')
  console.log('PASS: visible group menu supports keyboard split and focus restoration')

  await fresh(persisted, '?deleted=beta')
  await expectIds(['alpha'])
  await fresh({
    version: 1,
    engine: 'dockview',
    activePanelId: 'session:beta',
    layout: { grid: {}, panels: {} },
  })
  await expectIds(['alpha'])
  failRead = true
  await fresh(persisted)
  await expectIds(['alpha'])
  await open('Gamma', 'right')
  await expectIds(['alpha', 'gamma'])
  await page.getByRole('button', { name: 'Close Gamma', exact: true }).click()
  await expectIds(['alpha'])
  await page.evaluate(() => window.setFixtureMounted(false))
  await page.getByText('Unmounted', { exact: true }).waitFor()
  assert.equal(
    await page.evaluate(() => window.evidence.writeAttempts),
    0,
    'Failed layout reads must not overwrite the existing split on close or unmount',
  )
  assert.deepEqual(savedLayout, persisted)
  failRead = false
  await page.evaluate(() => window.setFixtureMounted(true))
  await checkDirection('below')
  await page.getByRole('button', { name: 'Close Beta', exact: true }).click()
  await expectIds(['alpha'])
  assert.ok(!(await saved()).layout.panels['session:beta'])
  console.log(
    'PASS: deleted sessions and malformed layouts fall back; failed reads preserve storage until re-entry succeeds',
  )

  readGate = Promise.withResolvers()
  await fresh(persisted)
  await open('Gamma', 'right')
  readGate.resolve()
  readGate = null
  await page.waitForFunction(
    () => document.querySelector('[data-active-id]')?.dataset.activeId === 'gamma',
  )
  await page.locator('[data-session-id="gamma"]').waitFor({ state: 'visible' })
  assert.ok((await saved()).layout.panels['session:gamma'])
  console.log('PASS: session selected while saved layout loads is replayed')

  await page.setViewportSize({ width: 825, height: 712 })
  for (const direction of ['left', 'right', 'above', 'below']) {
    await fresh(null, '?sidebar=1')
    await expectIds(['alpha'])
    await open('Beta')
    await expectIds(['beta'])
    await page.getByRole('button', { name: 'chat:focusSession.chatActions', exact: true }).click()
    const suffix = { left: 'Left', right: 'Right', above: 'Top', below: 'Bottom' }[direction]
    await page
      .getByRole('menuitem', { name: `chat:chatPage.splitTo${suffix}`, exact: true })
      .click()
    await checkDirection(direction)
    await checkBounds()
  }
  await fresh(null, '?sidebar=1')
  await expectIds(['alpha'])
  await open('Beta', 'right')
  const oldConstraints = await saved()
  for (const panel of Object.values(oldConstraints.layout.panels)) {
    panel.minimumWidth = 300
    panel.minimumHeight = 240
  }
  await fresh(oldConstraints, '?sidebar=1')
  await checkDirection('right')
  await checkBounds()
  console.log('PASS: narrow desktop retains four-way split actions and two visible panels')
  assert.deepEqual(errors, [], 'No browser exceptions')
  console.log('Chat Dock smoke passed')
} finally {
  readGate?.resolve()
  await browser?.close()
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
}
