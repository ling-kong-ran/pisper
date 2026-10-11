// 移动壳回归：真实 React 组件、隔离接口/桥接和本机浏览器，不启动模型或读取用户配置。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { readFile, readdir, mkdir, writeFile } from 'node:fs/promises'
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
      getSessionWorkflowRuns: (id, signal) => new Promise(resolve => {
        (window.workflowRequests ||= []).push({id,signal,resolve})
      }),
      getSessionFileChanges:async () => { window.evidence.reads++; return data() },
      approveSessionFileChanges:async () => { window.evidence.approvals++; return data() },
      revertSessionFileChanges:async () => { window.evidence.reverts++; return data() },
    }`,
  '@/features/chat/model/session-change-summary-api': `export async function invalidateSessionChangeSummary() {}`,
  '@/lib/http/api': `export async function apiJson(url, options) {
    if (url === '/api/desktop/reveal-path') { window.evidence.reveals.push(JSON.parse(options.body).path); return {revealed:true} }
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
  if (process.env.PISPER_MOBILE_PERF_REPORT) {
    const client = await page.context().newCDPSession(page)
    await client.send('Performance.enable')
    await page.evaluate(() => window.controls.setStress(true))
    await page.waitForFunction(() => document.querySelectorAll('[data-stress-row]').length === 2400)
    await client.send('Emulation.setCPUThrottlingRate', { rate: 4 })
    const samples = []
    for (let round = 0; round < 3; round++) {
      for (const target of ['sessions', 'context']) {
        await page.waitForFunction(() => document.getAnimations().length === 0)
        const before = (await client.send('Performance.getMetrics')).metrics
        await page.evaluate(() => {
          window.firstFlipFrame = null
          document.addEventListener(
            'click',
            () => {
              const start = performance.now()
              requestAnimationFrame(() => {
                window.firstFlipFrame = performance.now() - start
              })
            },
            { once: true, capture: true },
          )
        })
        const start = performance.now()
        await page
          .getByRole('tab', { name: `navigation:mobileShell.${target}`, exact: true })
          .click()
        await page.waitForFunction(
          () => document.getAnimations().length === 0 && window.firstFlipFrame !== null,
        )
        const after = (await client.send('Performance.getMetrics')).metrics
        const metric = (name) =>
          after.find((m) => m.name === name).value - before.find((m) => m.name === name).value
        samples.push({
          target,
          firstFrameMs: await page.evaluate(() => window.firstFlipFrame),
          elapsedMs: performance.now() - start,
          layouts: metric('LayoutCount'),
          layoutMs: metric('LayoutDuration') * 1000,
        })
      }
    }
    await writeFile(
      process.env.PISPER_MOBILE_PERF_REPORT,
      JSON.stringify(
        { browser: browser.version(), viewport: '390x844', rows: 2400, cpuThrottle: 4, samples },
        null,
        2,
      ),
    )
    console.log(JSON.stringify(samples))
    await client.send('Emulation.setCPUThrottlingRate', { rate: 1 })
    await client.send('Performance.disable')
    await page.evaluate(() => window.controls.setStress(false))
    await page.waitForFunction(() => document.querySelectorAll('[data-stress-row]').length === 0)
  }
  const openWorkspace = page.getByRole('button', {
    name: 'navigation:mobileShell.openWorkspace',
    exact: true,
  })
  const workspaceTabs = page.getByRole('tablist', {
    name: 'navigation:mobileShell.context',
    exact: true,
  })
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 })
    const before = await openWorkspace.boundingBox()
    assert.ok(
      before.x >= 0 && before.x + before.width <= width,
      'File action stays inside the viewport',
    )
    await workspaceTabs.evaluate((node) => {
      node.scrollLeft = node.scrollWidth
    })
    const after = await openWorkspace.boundingBox()
    assert.equal(
      Math.round(after.x),
      Math.round(before.x),
      'Scrolling workspace tabs must not move the file action',
    )
    await workspaceTabs.evaluate((node) => {
      node.scrollLeft = 0
    })
  }
  await page.setViewportSize({ width: 390, height: 844 })
  await openWorkspace.click()
  await page.waitForFunction(() => window.evidence.reveals.length === 1)
  assert.deepEqual(await page.evaluate(() => window.evidence.reveals), ['/fixture'])
  assert.equal(
    await page.getByRole('tab', { name: 'navigation:mobileShell.files', exact: true }).count(),
    0,
  )
  await page.evaluate(() => {
    window.evidence.cwd = ''
  })
  await openWorkspace.click()
  await page.waitForFunction(() => window.evidence.notices.length === 1)
  assert.equal(await page.evaluate(() => window.evidence.reveals.length), 1)
  await page.evaluate(() => {
    window.evidence.cwd = '/second-project'
  })
  await openWorkspace.click()
  await page.waitForFunction(() => window.evidence.reveals.length === 2)
  assert.deepEqual(await page.evaluate(() => window.evidence.reveals), [
    '/fixture',
    '/second-project',
  ])
  await page.evaluate(() => {
    window.evidence.holdCwd = true
  })
  await page.getByRole('button', { name: 'newTerminal', exact: true }).evaluate((button) => {
    button.click()
    button.click()
  })
  await page.waitForFunction(() => window.evidence.cwdRequests >= 1)
  assert.equal(
    await page.locator('.terminal-tab [data-status="starting"]').count(),
    1,
    'Directory lookup must show an immediate starting tab',
  )
  assert.equal(
    await page.evaluate(() => window.evidence.cwdRequests),
    1,
    'Pending creation must coalesce repeated clicks',
  )
  await page.evaluate(() => {
    window.releaseCwd('/second-project')
    window.evidence.holdCwd = false
  })
  await page.waitForFunction(() => window.evidence.created === 1)
  await page.evaluate(() => {
    window.evidence.holdCwd = true
  })
  await page.getByRole('button', { name: 'newTerminal', exact: true }).click()
  await page.waitForFunction(() => window.evidence.cwdRequests === 2)
  await page.evaluate(() => {
    window.rejectCwd(new Error('cwd-fixture-failed'))
    window.evidence.holdCwd = false
  })
  await page.locator('.terminal-tab [data-status="error"]').waitFor()
  assert.equal(
    await page.evaluate(() => window.evidence.created),
    1,
    'A failed directory lookup must not launch a shell in another directory',
  )
  await page
    .locator('.terminal-tab')
    .filter({ has: page.locator('[data-status="error"]') })
    .locator('svg[aria-label="closeTerminal"]')
    .click()
  await page.evaluate(() => {
    window.evidence.holdCwd = true
  })
  await page.getByRole('button', { name: 'newTerminal', exact: true }).click()
  await page.waitForFunction(() => window.evidence.cwdRequests === 3)
  await page
    .locator('.terminal-tab')
    .filter({ has: page.locator('[data-status="starting"]') })
    .locator('svg[aria-label="closeTerminal"]')
    .click()
  await page.evaluate(() => {
    window.releaseCwd('/cancelled-project')
    window.evidence.holdCwd = false
  })
  await page.getByRole('button', { name: 'newTerminal', exact: true }).click()
  await page.waitForFunction(() => window.evidence.created === 2)
  assert.equal(
    await page.locator('.terminal-tab').count(),
    2,
    'Closed pending terminal must not return after its directory resolves',
  )
  await page.locator('.terminal-tab').last().locator('svg[aria-label="closeTerminal"]').click()
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
    ['touchStart', [{ x: 270, y: 450 }]],
    ['touchMove', [{ x: 150, y: 450 }]],
    ['touchMove', [{ x: 80, y: 450 }]],
    ['touchEnd', []],
  ])
    await cdp.send('Input.dispatchTouchEvent', { type, touchPoints: points })
  await page.waitForFunction(
    () => !document.querySelector('section[aria-label="navigation:mobileShell.context"]').inert,
  )
  const currentPane = () =>
    page.evaluate(() =>
      document.querySelector('[role="tabpanel"]:not([inert])')?.getAttribute('aria-label'),
    )
  const settle = () => page.waitForFunction(() => document.getAnimations().length === 0)
  const swipe = async (start, end) => {
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchStart',
      touchPoints: [{ x: start[0], y: start[1] }],
    })
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [{ x: (start[0] + end[0]) / 2, y: (start[1] + end[1]) / 2 }],
    })
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [{ x: end[0], y: end[1] }],
    })
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
    await settle()
  }
  await settle()
  const previousPane = page.getByRole('button', {
    name: 'navigation:mobileShell.previousPane',
    exact: true,
  })
  const nextPane = page.getByRole('button', {
    name: 'navigation:mobileShell.nextPane',
    exact: true,
  })
  assert.equal(await nextPane.isDisabled(), true)
  assert.equal(await previousPane.textContent(), '', 'Navigation arrows have no visible text')
  assert.equal(await nextPane.textContent(), '')
  await previousPane.click()
  await settle()
  assert.equal(await currentPane(), 'navigation:mobileShell.chat')
  await previousPane.click()
  await settle()
  assert.equal(await currentPane(), 'navigation:mobileShell.sessions')
  assert.equal(await previousPane.isDisabled(), true)
  await nextPane.click()
  await settle()
  assert.equal(await currentPane(), 'navigation:mobileShell.chat')
  await page.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).click()
  await settle()
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [{ x: 300, y: 450 }],
  })
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchMove',
    touchPoints: [{ x: 180, y: 450 }],
  })
  await page.waitForFunction(() => {
    const panel = document.querySelector(
      '[aria-label="navigation:mobileShell.chat"][role="tabpanel"]',
    )
    return getComputedStyle(panel).transform.startsWith('matrix3d(')
  })
  const during = await page
    .locator('[aria-label="navigation:mobileShell.chat"][role="tabpanel"]')
    .boundingBox()
  assert.ok(during.x < 0, 'Horizontal drag must visibly follow the finger')
  await mkdir(join(root, 'release/mobile-flip-verification'), { recursive: true })
  await page.screenshot({ path: join(root, 'release/mobile-flip-verification/dragging.png') })
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await settle()
  assert.equal(await currentPane(), 'navigation:mobileShell.context')
  await swipe([80, 450], [290, 450])
  assert.equal(await currentPane(), 'navigation:mobileShell.chat')
  await swipe([80, 450], [290, 450])
  assert.equal(await currentPane(), 'navigation:mobileShell.sessions')
  await swipe([80, 450], [290, 450])
  assert.equal(await currentPane(), 'navigation:mobileShell.sessions', 'First pane does not wrap')
  await swipe([300, 450], [80, 450])
  assert.equal(await currentPane(), 'navigation:mobileShell.chat')
  await swipe([220, 450], [195, 450])
  assert.equal(await currentPane(), 'navigation:mobileShell.chat', 'Short swipes snap back')

  for (const label of ['composer', 'horizontal-content', 'embedded-control']) {
    const box = await page.getByLabel(label, { exact: true }).boundingBox()
    await swipe(
      [box.x + Math.min(box.width - 10, 180), box.y + box.height / 2],
      [box.x + 20, box.y + box.height / 2],
    )
    assert.equal(await currentPane(), 'navigation:mobileShell.chat', `${label} owns its gesture`)
  }
  await page.getByLabel('composer').blur()
  const scroll = await page.getByLabel('vertical-content').boundingBox()
  await swipe([scroll.x + 100, scroll.y + 130], [scroll.x + 100, scroll.y + 20])
  assert.equal(await currentPane(), 'navigation:mobileShell.chat')
  assert.ok(
    await page.getByLabel('vertical-content').evaluate((node) => node.scrollTop > 0),
    'Vertical scrolling remains native',
  )

  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [{ x: 300, y: 450 }],
  })
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchMove',
    touchPoints: [{ x: 180, y: 450 }],
  })
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchCancel', touchPoints: [] })
  await settle()
  assert.equal(
    await currentPane(),
    'navigation:mobileShell.chat',
    'Cancelled gestures do not navigate',
  )
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [{ x: 300, y: 450, id: 0 }],
  })
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchStart',
    touchPoints: [
      { x: 300, y: 450, id: 0 },
      { x: 280, y: 480, id: 1 },
    ],
  })
  await cdp.send('Input.dispatchTouchEvent', {
    type: 'touchMove',
    touchPoints: [
      { x: 100, y: 450, id: 0 },
      { x: 80, y: 480, id: 1 },
    ],
  })
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] })
  await settle()
  assert.equal(await currentPane(), 'navigation:mobileShell.chat', 'Pinch gestures do not navigate')

  await page.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).focus()
  await page.keyboard.press('ArrowLeft')
  await page.keyboard.press('ArrowRight')
  await page.keyboard.press('ArrowRight')
  await settle()
  assert.equal(
    await currentPane(),
    'navigation:mobileShell.context',
    'Rapid keyboard changes settle on the latest pane',
  )
  await page.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).click()
  await settle()
  const fixed = await page.locator('[data-fixed-probe]').boundingBox()
  assert.equal(Math.round(fixed.x), 380)
  assert.equal(Math.round(fixed.y), 834)
  assert.ok(
    await page
      .locator('[role="tabpanel"]')
      .evaluateAll((nodes) => nodes.every((node) => getComputedStyle(node).transform === 'none')),
    'Resting panels do not create fixed-position containing blocks',
  )
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.getByRole('tab', { name: 'navigation:mobileShell.context', exact: true }).click()
  assert.equal(
    await page.evaluate(() => {
      const panels = [...document.querySelectorAll('[data-mobile-shell] > div > section')]
      return [...panels, panels[0]?.parentElement]
        .filter(Boolean)
        .flatMap((node) => node.getAnimations())
        .filter(
          (animation) =>
            Number(animation.effect.getTiming().duration) > 1 &&
            animation.effect
              .getKeyframes()
              .some((frame) => 'transform' in frame || 'marginLeft' in frame),
        ).length
    }),
    0,
    'Reduced motion skips flip transforms',
  )
  await page.emulateMedia({ reducedMotion: 'no-preference' })
  await page.evaluate(() => (document.documentElement.dataset.motion = 'reduced'))
  await page.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).click()
  assert.equal(
    await page.evaluate(() => {
      const panels = [...document.querySelectorAll('[data-mobile-shell] > div > section')]
      return [...panels, panels[0]?.parentElement]
        .filter(Boolean)
        .flatMap((node) => node.getAnimations())
        .filter(
          (animation) =>
            Number(animation.effect.getTiming().duration) > 1 &&
            animation.effect
              .getKeyframes()
              .some((frame) => 'transform' in frame || 'marginLeft' in frame),
        ).length
    }),
    0,
    'App motion preference skips flips',
  )
  await page.evaluate(() => delete document.documentElement.dataset.motion)
  await page.getByRole('tab', { name: 'navigation:mobileShell.context', exact: true }).click()
  await page.evaluate(() => window.controls.setMode('pad'))
  await page.waitForFunction(() => document.querySelector('[data-mobile-shell="pad"]'))
  assert.equal(
    await page.evaluate(() => {
      const panels = [...document.querySelectorAll('[data-mobile-shell] > div > section')]
      return [...panels, panels[0]?.parentElement]
        .filter(Boolean)
        .flatMap((node) => node.getAnimations())
        .filter(
          (animation) =>
            Number(animation.effect.getTiming().duration) > 1 &&
            animation.effect
              .getKeyframes()
              .some((frame) => 'transform' in frame || 'marginLeft' in frame),
        ).length
    }),
    0,
    'Layout changes cancel pending flips',
  )
  assert.ok(
    await page
      .locator('[data-mobile-shell] > div > section')
      .evaluateAll((nodes) => nodes.every((node) => getComputedStyle(node).transform === 'none')),
  )
  await page.evaluate(() => {
    window.controls.setMode('phone')
    window.controls.setPane('context')
  })
  await page.waitForFunction(() => document.querySelector('[data-mobile-shell="phone"]'))
  await settle()
  for (const target of ['sessions', 'chat', 'context', 'chat', 'sessions', 'context', 'sessions']) {
    await page.getByRole('tab', { name: `navigation:mobileShell.${target}`, exact: true }).click()
    assert.equal(
      await page.evaluate(() => window.evidence.flipStartedBeforePaneUpdate),
      true,
      'Click must start the flip before notifying the parent React tree',
    )
    assert.ok(
      await page.evaluate(() => {
        const panels = [...document.querySelectorAll('[data-mobile-shell] > div > section')]
        return [...panels, panels[0]?.parentElement]
          .filter(Boolean)
          .flatMap((node) => node.getAnimations())
          .filter((animation) => Number(animation.effect.getTiming().duration) > 1)
          .every((animation) =>
            animation.effect
              .getKeyframes()
              .every(
                (frame) => !('marginLeft' in frame) && !('left' in frame) && !('width' in frame),
              ),
          )
      }),
      'Flip frames must not animate layout properties',
    )
    await settle()
    assert.equal(await currentPane(), `navigation:mobileShell.${target}`)
    assert.ok(
      await page.evaluate(() => {
        const panels = [...document.querySelectorAll('[data-mobile-shell] > div > section')]
        return [...panels, panels[0]?.parentElement].every(
          (node) =>
            getComputedStyle(node).transform === 'none' &&
            getComputedStyle(node).willChange === 'auto',
        )
      }),
      'Finished flips must release layers and fixed-position containing blocks',
    )
  }
  await page.getByRole('tab', { name: 'navigation:mobileShell.context', exact: true }).click()
  await page.evaluate(() => {
    document.querySelectorAll('[role=tabpanel]').forEach((node) =>
      node
        .getAnimations()
        .filter((animation) =>
          animation.effect.getKeyframes().some((frame) => 'transform' in frame),
        )
        .forEach((animation) => {
          animation.pause()
          animation.currentTime = 1
        }),
    )
  })
  await page.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).click()
  assert.ok(
    await page.evaluate(() => {
      const track = document.querySelector('[role=tabpanel]').parentElement
      const animation = track
        .getAnimations()
        .find((a) => a.effect.getKeyframes().some((frame) => 'transform' in frame))
      return new DOMMatrixReadOnly(animation.effect.getKeyframes()[0].transform).m41 > 0
    }),
    'An early interrupted cross-pane flip starts from the visible source, not the unseen destination',
  )
  await settle()
  await page.getByRole('tab', { name: 'navigation:mobileShell.context', exact: true }).click()
  await settle()
  await page.evaluate(() => {
    window.evidence.holdPaneUpdates = true
  })
  await previousPane.click()
  await settle()
  await swipe([300, 450], [80, 450])
  assert.equal(
    await page.evaluate(() => window.evidence.pendingPane),
    'context',
    'Reversing a flip before the parent commits must supersede the previous request',
  )
  await previousPane.click()
  await previousPane.click()
  assert.equal(
    await page.evaluate(() => window.evidence.pendingPane),
    'sessions',
    'Rapid arrows must advance from the pending target',
  )
  await page.evaluate(() => {
    window.evidence.holdPaneUpdates = false
    window.controls.setPane(window.evidence.pendingPane)
  })
  await settle()
  assert.equal(await currentPane(), 'navigation:mobileShell.sessions')
  assert.equal(await page.evaluate(() => window.evidence.closeAll), 0)
  await page.evaluate(() => window.controls.setMounted(false))
  await page.waitForFunction(() => window.evidence.closeAll === 1)
  await page.clock.install()
  await page.evaluate(() => {
    window.controls.setMounted(true)
    window.controls.setMode('workflow')
    window.controls.setSession('workflow-one')
  })
  await page.waitForFunction(() => window.workflowRequests?.length === 1)
  await page.clock.fastForward(5000)
  assert.equal(
    await page.evaluate(() => window.workflowRequests.length),
    1,
    'Slow workflow requests must not overlap',
  )
  await page.evaluate(() => window.controls.setSession('workflow-two'))
  await page.waitForFunction(() => window.workflowRequests.length === 2)
  assert.equal(await page.evaluate(() => window.workflowRequests[0].signal.aborted), true)
  await page.evaluate(() => {
    const run = (name) => ({
      runs: [{ id: name, workflowName: name, status: 'completed', nodes: [] }],
    })
    window.workflowRequests[1].resolve(run('current-workflow'))
    window.workflowRequests[0].resolve(run('stale-workflow'))
  })
  await page.clock.fastForward(1)
  await page.getByText('current-workflow', { exact: true }).waitFor()
  assert.equal(await page.getByText('stale-workflow', { exact: true }).count(), 0)
  await page.clock.fastForward(1600)
  await page.waitForFunction(() => window.workflowRequests.length === 3)
  await page.evaluate(() => window.controls.setMounted(false))
  assert.equal(await page.evaluate(() => window.workflowRequests[2].signal.aborted), true)
  assert.deepEqual(errors, [])
  console.log(
    'PASS mobile shell: terminal lifetime, independent save, run safety, refresh, layout, swipe/flip gestures, reduced motion and workflow polling isolation',
  )
} finally {
  await browser?.close()
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
}
