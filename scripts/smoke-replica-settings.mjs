import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp, readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { createHash } from 'node:crypto'
import { tmpdir } from 'node:os'
import { createServer } from 'node:http'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const output = await mkdtemp(join(tmpdir(), 'pisper-replica-settings-'))
const assetDirectory = join(root, 'dist/assets')
const assetNames = new Set(await readdir(assetDirectory))
const cssNames = [...assetNames].filter((name) => name.endsWith('.css')).sort()
assert.ok(
  cssNames.some((name) => /^index-.*\.css$/.test(name)),
  'Build production CSS before running settings verification',
)
const cssFiles = await Promise.all(
  cssNames.map(async (name) => {
    const content = await readFile(join(assetDirectory, name), 'utf8')
    return {
      name,
      content,
      bytes: Buffer.byteLength(content),
      sha256: createHash('sha256').update(content).digest('hex'),
      modifiedAt: (await stat(join(assetDirectory, name))).mtime.toISOString(),
    }
  }),
)
const css = cssFiles.map(({ content }) => content).join('\n')
const metrics = {
  generatedAt: new Date().toISOString(),
  capture: {
    css: 'all current dist/assets/*.css, sorted by filename',
    animations: 'disabled for screenshots; finite popover animations settled before measurement',
    reducedMotion: 'reduce',
    fixtureSurface: 'real PageFrame settings content, without App shell',
  },
  browser: null,
  cssFiles: cssFiles.map(({ content: _content, ...metadata }) => metadata),
  scenarios: [],
  findings: [],
  externalRequests: [],
}
const bundle = await build({
  entryPoints: [join(root, 'scripts/fixtures/replica-settings.tsx')],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'browser',
  jsx: 'automatic',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  loader: { '.css': 'empty' },
})
const server = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://fixture').pathname
  if (pathname === '/fixture.js') {
    response.setHeader('Content-Type', 'text/javascript')
    response.end(bundle.outputFiles[0].text)
  } else if (pathname === '/assets/fixture.css') {
    response.setHeader('Content-Type', 'text/css')
    response.end(css)
  } else if (pathname.startsWith('/assets/')) {
    const name = pathname.slice('/assets/'.length)
    if (!assetNames.has(name)) {
      response.writeHead(404)
      response.end()
      return
    }
    response.setHeader(
      'Content-Type',
      name.endsWith('.woff2')
        ? 'font/woff2'
        : name.endsWith('.woff')
          ? 'font/woff'
          : 'application/octet-stream',
    )
    response.end(await readFile(join(assetDirectory, name)))
  } else {
    response.setHeader('Content-Type', 'text/html')
    response.end(
      '<!doctype html><html lang="zh-CN"><head><meta name="viewport" content="width=device-width,initial-scale=1"/><link rel="stylesheet" href="/assets/fixture.css"/></head><body><div id="root"></div><script type="module" src="/fixture.js"></script></body></html>',
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
      '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    ].find(existsSync)
  assert(executablePath, 'Set PISPER_SMOKE_BROWSER to an installed Chromium executable.')
  browser = await chromium.launch({ executablePath, headless: true })
  metrics.browser = browser.version()
  const page = await browser.newPage({
    viewport: { width: 1280, height: 900 },
    deviceScaleFactor: 1,
    reducedMotion: 'reduce',
  })
  page.setDefaultTimeout(4000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  const url = `http://127.0.0.1:${server.address().port}`
  await page.route('**/*', (route) => {
    if (new URL(route.request().url()).origin !== url) {
      metrics.externalRequests.push(route.request().url())
      return route.abort()
    }
    return route.fallback()
  })
  let reads = 0
  const writes = []
  let failRead = true
  let failWrite = false
  let releaseWrite
  await page.route('**/api/settings/memory', async (route) => {
    if (route.request().method() === 'PATCH') {
      writes.push(route.request().postDataJSON())
      await new Promise((resolve) => {
        releaseWrite = resolve
      })
      return route.fulfill(
        failWrite
          ? { status: 503, json: { error: 'Fixture write unavailable' } }
          : { json: { autoApproveConfidence: writes.at(-1).autoApproveConfidence } },
      )
    }
    reads++
    if (failRead) return route.fulfill({ status: 503, json: { error: 'Fixture unavailable' } })
    return route.fulfill({ json: { autoApproveConfidence: 75 } })
  })
  await page.route('**/api/memory?*', (route) =>
    route.fulfill({
      json: {
        spaces: [],
        nodes: [],
        links: [],
        candidates: [],
        selectedSpaceId: '',
      },
    }),
  )
  await page.goto(url)
  const threshold = page.getByRole('spinbutton')
  await page.getByRole('button', { name: /重试|Retry/ }).waitFor()
  assert.equal(await threshold.count(), 0, 'A failed read must not invent an editable threshold')
  assert.equal(writes.length, 0)
  failRead = false
  await page.getByRole('button', { name: /重试|Retry/ }).click()
  await threshold.waitFor().catch(async (error) => {
    console.error({ reads, errors, body: await page.locator('body').innerText() })
    throw error
  })
  assert.equal(await threshold.inputValue(), '75')
  await threshold.focus()
  await threshold.blur()
  assert.equal(writes.length, 0, 'Unchanged blur must not PATCH')
  const beforeUnsubmittedDraft = await page.evaluate(() => window.replicaNotices.length)
  await threshold.fill('81')
  await threshold.press('Enter')
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'true',
  )
  await threshold.fill('88')
  releaseWrite()
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'false',
  )
  assert.equal(await threshold.inputValue(), '88', 'Late save response must preserve newer edits')
  assert.deepEqual(writes, [{ autoApproveConfidence: 81 }])
  assert.equal(
    await page.evaluate(() => window.replicaNotices.length),
    beforeUnsubmittedDraft,
    'An older successful write cannot announce a newer unsubmitted draft as saved',
  )
  assert.equal(reads, 2)
  console.log('PASS memory read failure/retry, unchanged blur, and edits during pending save')

  failWrite = true
  await threshold.press('Enter')
  await threshold.blur()
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'true',
  )
  releaseWrite()
  await page.getByRole('button', { name: /重试|Retry/ }).waitFor()
  assert.equal(await threshold.inputValue(), '88', 'A failed save keeps the draft')
  failWrite = false
  await page.getByRole('button', { name: /重试|Retry/ }).click()
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'true',
  )
  releaseWrite()
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'false',
  )
  assert.deepEqual(writes, [
    { autoApproveConfidence: 81 },
    { autoApproveConfidence: 88 },
    { autoApproveConfidence: 88 },
  ])
  console.log('PASS failed memory save keeps the draft and Retry repeats the write')

  // 当前 PATCH 完成前的 blur/Enter 都是新的明确提交意图，不能只保留显示草稿。
  await threshold.fill('81')
  await threshold.press('Enter')
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'true',
  )
  await threshold.fill('85')
  await threshold.press('Enter')
  await threshold.fill('88')
  await threshold.blur()
  const beforeQueuedSuccess = await page.evaluate(() => window.replicaNotices.length)
  const queuedRequest = page.waitForRequest(
    (request) =>
      request.method() === 'PATCH' && request.postDataJSON().autoApproveConfidence === 88,
  )
  releaseWrite()
  await queuedRequest
  assert.equal(await threshold.getAttribute('aria-busy'), 'true')
  assert.equal(
    await page.evaluate(() => window.replicaNotices.length),
    beforeQueuedSuccess,
    'An older successful write cannot announce the newer draft as saved',
  )
  assert.deepEqual(
    writes.slice(-2),
    [{ autoApproveConfidence: 81 }, { autoApproveConfidence: 88 }],
    'Only the latest pending explicit submission is written',
  )
  releaseWrite()
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'false',
  )
  assert.equal(await threshold.inputValue(), '88')
  assert.equal(await page.evaluate(() => window.replicaNotices.length), beforeQueuedSuccess + 1)
  console.log(
    'PASS submissions made during a pending save are coalesced and persisted before success is announced',
  )

  const beforeFailedQueueWrites = writes.length
  const beforeFailedQueueNotices = await page.evaluate(() => window.replicaNotices.length)
  failWrite = true
  await threshold.fill('81')
  await threshold.press('Enter')
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'true',
  )
  await threshold.fill('88')
  await threshold.press('Enter')
  await threshold.blur()
  releaseWrite()
  await page.getByRole('button', { name: /重试|Retry/ }).waitFor()
  assert.equal(
    writes.length,
    beforeFailedQueueWrites + 1,
    'A failed write must pause the queue until explicit retry',
  )
  assert.equal(await threshold.inputValue(), '88')
  assert.equal(await page.evaluate(() => window.replicaNotices.length), beforeFailedQueueNotices)
  failWrite = false
  const retryLatest = page.waitForRequest(
    (request) =>
      request.method() === 'PATCH' && request.postDataJSON().autoApproveConfidence === 88,
  )
  await page.getByRole('button', { name: /重试|Retry/ }).click()
  await retryLatest
  releaseWrite()
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'false',
  )
  assert.deepEqual(writes.slice(-2), [{ autoApproveConfidence: 81 }, { autoApproveConfidence: 88 }])
  console.log(
    'PASS failed in-flight write pauses newer intent; Retry confirms the latest value even when it equals an older snapshot',
  )

  await threshold.fill('81')
  await threshold.press('Enter')
  await page.waitForFunction(
    () => document.querySelector('input[type=number]')?.getAttribute('aria-busy') === 'true',
  )
  await threshold.fill('88')
  await threshold.press('Enter')
  const beforeUnmountWrites = writes.length
  const beforeUnmountNotices = await page.evaluate(() => window.replicaNotices.length)
  const cancelledWrite = page.waitForEvent('requestfailed', {
    predicate: (request) => request.method() === 'PATCH',
  })
  await page.getByRole('button', { name: 'Unmount memory' }).click()
  const aborted = await cancelledWrite
  assert.ok(aborted.failure())
  releaseWrite()
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(writes.length, beforeUnmountWrites, 'Unmount must cancel queued writes')
  assert.equal(
    await page.evaluate(() => window.replicaNotices.length),
    beforeUnmountNotices,
    'Unmount must suppress stale success feedback',
  )
  console.log('PASS unmount aborts the active request and discards queued submission intent')

  await page.goto(`${url}/?view=search`)
  await page.getByRole('button', { name: 'Search policy' }).click()
  await page.getByRole('button', { name: 'Finish loading' }).click()
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('aria-label') === 'Runtime policy',
  )
  await page.reload()
  await page.getByRole('button', { name: 'Search policy' }).click()
  await page.getByRole('button', { name: 'Search current' }).click()
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('aria-label') === 'Current model',
  )
  await page.getByRole('button', { name: 'Finish loading' }).click()
  await page.getByRole('textbox', { name: 'Runtime policy' }).waitFor()
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.notEqual(
    await page.evaluate(() => document.activeElement?.getAttribute('aria-label')),
    'Runtime policy',
    'Late content cannot consume an older search request',
  )
  await page.getByRole('button', { name: 'Search policy' }).click()
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('aria-label') === 'Runtime policy',
  )
  console.log('PASS search expansion, delayed mounting, latest navigation, and repeat navigation')
  const config = { providers: [], provider: '', model: '', thinkingLevel: 'off', toolMode: 'ask' }
  let releaseConfig
  await page.route('**/api/config', async (route) => {
    await new Promise((resolve) => {
      releaseConfig = resolve
    })
    await route.fulfill({ json: config })
  })
  await page.route('**/api/providers/models/refresh', (route) =>
    route.fulfill({ json: { config } }),
  )
  await page.route('**/api/providers/import-local', (route) =>
    route.fulfill({
      json: { config, discovery: { providers: [], errors: [] }, imported: [], skipped: [] },
    }),
  )
  await page.route('**/api/visual/models', (route) =>
    route.fulfill({ json: { imageModels: [], videoModels: [] } }),
  )
  await page.goto(`${url}/?view=models`)
  await page.getByRole('button', { name: 'Locate real policy' }).click()
  assert.ok(releaseConfig)
  releaseConfig()
  const policyTarget = page.locator('[data-config-target="models-runtime-policy"]')
  await policyTarget.waitFor()
  await page.waitForFunction(() =>
    Boolean(document.activeElement?.closest('[data-config-target="models-runtime-policy"]')),
  )
  await page.locator('[data-config-card="models-advanced"] > button').click()
  await page.getByRole('button', { name: 'Locate real visual' }).click()
  await page.waitForFunction(() =>
    Boolean(document.activeElement?.closest('[data-config-card="models-visual"]')),
  )
  console.log('PASS real ModelsSettings policy and visual expansion after delayed config loading')

  await page.addInitScript(() => {
    if (location.search.includes('appearance'))
      localStorage.setItem(
        'pisper-ui',
        JSON.stringify({
          state: { theme: 'system', accent: 'custom', customAccent: '#1677e8' },
          version: 1,
        }),
      )
  })
  await page.goto(`${url}/?view=appearance`)
  const modes = page.getByRole('radiogroup', { name: /颜色模式|Color mode/ })
  assert.equal(await modes.getByRole('radio').count(), 3)
  const modeValues = await modes
    .getByRole('radio')
    .evaluateAll((elements) => elements.map((element) => element.value))
  assert.deepEqual(modeValues, ['system', 'light', 'dark'])
  await modes.getByRole('radio').nth(0).focus()
  await modes.getByRole('radio').nth(0).press('ArrowRight')
  assert.equal(await modes.getByRole('radio').nth(1).isChecked(), true)
  const advanced = page.getByRole('button', { name: /高级|Advanced/, exact: true })
  assert.equal(await advanced.getAttribute('aria-expanded'), 'false')
  await advanced.click()
  assert.equal(await advanced.getAttribute('aria-expanded'), 'true')
  const controls = await advanced.getAttribute('aria-controls')
  assert.ok(controls)
  assert.equal(await page.locator(`[id="${controls}"]`).count(), 1)
  await page.getByRole('button', { name: /自定义强调色|Custom accent color/, exact: true }).click()
  const hex = page.getByRole('textbox', { name: /HEX/ })
  await hex.fill('#12')
  await hex.press('Tab')
  const storedAccent = () =>
    page.evaluate(() => JSON.parse(localStorage.getItem('pisper-ui')).state.customAccent)
  assert.equal(await storedAccent(), '#1677e8', 'Partial HEX must never be persisted')
  await hex.fill('#123456')
  assert.equal(await storedAccent(), '#1677e8', 'Typing only changes the draft')
  await hex.press('Enter')
  assert.equal(await storedAccent(), '#123456')
  await hex.fill('#abc')
  await hex.press('Escape')
  assert.equal(await storedAccent(), '#123456')
  console.log(
    'PASS appearance native radio keyboard control, Advanced association, and complete HEX commit boundaries',
  )
  await page.goto(`${url}/?view=typography`)
  await page.getByRole('button', { name: /高级|Advanced/, exact: true }).click()
  const uiSize = page.getByRole('spinbutton', { name: /界面字号|UI font size/, exact: true })
  const codeSize = page.getByRole('spinbutton', { name: /代码字号|Code font size/, exact: true })
  await uiSize.waitFor()
  await page.evaluate(() => {
    window.__fontWriteCount = 0
    const original = Storage.prototype.setItem
    Storage.prototype.setItem = function (key, value) {
      if (key === 'pisper-ui') window.__fontWriteCount++
      return original.call(this, key, value)
    }
  })
  const storedTypography = () =>
    page.evaluate(() => JSON.parse(localStorage.getItem('pisper-ui')).state)
  await uiSize.fill('16')
  assert.equal((await storedTypography()).uiFontSize, 14, 'typing does not persist')
  await uiSize.press('Enter')
  await uiSize.press('Tab')
  assert.equal((await storedTypography()).uiFontSize, 16)
  assert.equal(await page.evaluate(() => window.__fontWriteCount), 1, 'Enter + blur commits once')
  await uiSize.fill('17')
  await uiSize.press('Enter')
  assert.equal((await storedTypography()).uiFontSize, 16)
  assert.equal(await uiSize.getAttribute('aria-invalid'), 'true')
  await uiSize.press('Escape')
  assert.equal(await uiSize.inputValue(), '16')
  await codeSize.fill('24')
  await codeSize.press('Enter')
  assert.equal((await storedTypography()).codeFontSize, 24)
  await page.waitForFunction(() => getComputedStyle(document.body).fontSize === '16px')
  assert.equal(
    await page
      .locator('[data-testid=typography-sample]')
      .evaluate((element) => getComputedStyle(element).fontSize),
    '16px',
  )
  await page.reload()
  await page.getByRole('button', { name: /高级|Advanced/, exact: true }).click()
  assert.equal(await uiSize.inputValue(), '16')
  assert.equal(await codeSize.inputValue(), '24')
  await uiSize.fill('11')
  await uiSize.press('Enter')
  await codeSize.fill('8')
  await codeSize.press('Enter')
  await page.waitForFunction(() => getComputedStyle(document.body).fontSize === '11px')
  assert.equal(
    await page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
    '16px',
  )
  console.log(
    'PASS independent numeric typography, draft validation, one commit and reload recovery',
  )

  // 第 4 期：界面页把“独立组件”标签换成跳到组件页的链接（旧 ?view=widgets 重定向过去）；
  // 强调色第一项是默认的“石墨”，选别的再选回来都只写一次偏好。
  // 前面的外观段落写过自定义强调色：清掉界面偏好，从全新设备的默认值开始。
  await page.evaluate(() => localStorage.removeItem('pisper-ui'))
  await page.goto(`${url}/?view=interface-page`)
  const moved = page.locator('[data-components-moved]')
  await moved.waitFor()
  assert.match(await moved.innerText(), /组件已移到 组件 页 →/)
  assert.equal(await page.getByRole('tab').count(), 0, 'settings no longer has a widgets tab')
  assert.ok((await moved.boundingBox()).height >= 44, 'the moved link is a 44px target')
  const accent = page.getByRole('combobox', { name: '强调色', exact: true })
  assert.match(await accent.innerText(), /石墨/, 'graphite is the default accent')
  await accent.click()
  const accentOptions = await page.getByRole('option').allInnerTexts()
  assert.equal(accentOptions[0].trim(), '石墨', 'graphite is the first accent preset')
  await page.getByRole('option', { name: 'Pisper 蓝', exact: true }).click()
  const storedUi = () => page.evaluate(() => JSON.parse(localStorage.getItem('pisper-ui')).state)
  assert.equal((await storedUi()).accent, 'blue')
  await accent.click()
  await page.getByRole('option', { name: '石墨', exact: true }).click()
  assert.equal((await storedUi()).accent, 'neutral')
  await moved.click()
  await page.getByTestId('components-route').waitFor()
  await page.goto(`${url}/?view=interface-page&legacy=widgets`)
  await page.getByTestId('components-route').waitFor()
  console.log(
    'PASS interface settings: the components-moved link replaces the widgets tab (legacy ?view=widgets redirects), graphite is the first and default accent and persists',
  )

  // 第 4 期快捷键：全局 / 看板 / 会话三组，固定键只读；新动作可改绑、冲突与不可用组合被拒、恢复默认。
  await page.goto(`${url}/?view=shortcuts`)
  const shortcutCard = page.locator('[data-config-card="shortcuts-bindings"]')
  await shortcutCard.waitFor()
  assert.deepEqual(
    await shortcutCard
      .locator('[data-shortcut-group]')
      .evaluateAll((nodes) => nodes.map((node) => node.getAttribute('data-shortcut-group'))),
    ['global', 'board', 'session'],
  )
  assert.equal(await shortcutCard.locator('[data-shortcut-fixed]').count(), 6)
  assert.equal(
    await shortcutCard.locator('[data-shortcut-fixed] [data-shortcut-recorder]').count(),
    0,
    'fixed keys are listed read-only',
  )
  for (const action of [
    'agentPanel',
    'toggleInspector',
    'previousAgent',
    'nextAgent',
    'steerMessage',
    'shortcutHelp',
    'showBoard',
    'boardApprove',
  ])
    assert.equal(await shortcutCard.locator(`#shortcut-${action}`).count(), 1, action)
  const storedShortcuts = () =>
    page.evaluate(() => JSON.parse(localStorage.getItem('pisper-shortcuts') || '{}'))
  const recordShortcut = async (action, keys) => {
    await shortcutCard.locator(`#shortcut-${action}`).click()
    await page.keyboard.press(keys)
  }
  await recordShortcut('agentPanel', 'F6')
  await page.waitForFunction(
    () => document.querySelector('#shortcut-agentPanel')?.textContent.replace(/\s+/g, '') === 'F6',
  )
  assert.equal((await storedShortcuts()).agentPanel, 'F6')
  await recordShortcut('nextAgent', 'Alt+ArrowUp')
  const nextAgentError = shortcutCard.locator('#shortcut-error-nextAgent')
  await nextAgentError.waitFor()
  assert.match(await nextAgentError.innerText(), /切换到上一个代理/)
  await page.keyboard.press('Escape')
  await recordShortcut('nextAgent', 'Control+ArrowDown')
  await page.waitForFunction(() =>
    /不可用/.test(document.querySelector('#shortcut-error-nextAgent')?.textContent || ''),
  )
  await page.keyboard.press('Escape')
  assert.equal(
    (await storedShortcuts()).nextAgent,
    'Alt+ArrowDown',
    'rejected recordings keep the previous binding',
  )
  await shortcutCard.getByRole('button', { name: '恢复默认', exact: true }).click()
  await page.waitForFunction(
    () => JSON.parse(localStorage.getItem('pisper-shortcuts') || '{}').agentPanel === 'Mod+KeyJ',
  )
  console.log(
    'PASS shortcut settings: global/board/session groups with six read-only fixed keys, new actions rebind and persist, conflicts and reserved arrow combinations are refused, reset restores defaults',
  )
  for (const scenario of [
    { name: 'light-1280', width: 1280, theme: 'light', longLabels: false },
    { name: 'dark-1280', width: 1280, theme: 'dark', longLabels: false },
    { name: 'light-390', width: 390, theme: 'light', longLabels: false },
    { name: 'dark-390', width: 390, theme: 'dark', longLabels: false },
    { name: 'dark-390-long-zh', width: 390, theme: 'dark', longLabels: true },
  ]) {
    await page.setViewportSize({ width: scenario.width, height: 900 })
    await page.goto(`${url}/?view=appearance${scenario.longLabels ? '&longLabels=1' : ''}`)
    await page.locator(`input[type=radio][value="${scenario.theme}"]`).locator('..').click()
    await page.waitForFunction(
      (theme) => document.documentElement.dataset.theme === theme,
      scenario.theme,
    )
    await page.evaluate(() => document.fonts.ready)
    const closedScreenshot = join(output, `appearance-${scenario.name}.png`)
    await page.screenshot({ path: closedScreenshot, fullPage: true, animations: 'disabled' })
    const capsule = page.getByRole('button', {
      name: /自定义强调色|Custom accent color/,
      exact: true,
    })
    await capsule.click()
    const popover = page.locator('[data-slot=popover-content]')
    await popover.waitFor()
    const focusOnOpen = await popover.evaluate((element) =>
      element.contains(document.activeElement),
    )
    const focusedControl = () =>
      page.evaluate(() => ({
        tag: document.activeElement?.tagName,
        role: document.activeElement?.getAttribute('role'),
        label: document.activeElement?.getAttribute('aria-label'),
        type: document.activeElement?.getAttribute('type'),
      }))
    const focusSequence = [await focusedControl()]
    await page.keyboard.press('Tab')
    focusSequence.push(await focusedControl())
    await page.keyboard.press('Tab')
    focusSequence.push(await focusedControl())
    await page.getByRole('textbox', { name: /HEX/ }).focus()
    await page.getByRole('textbox', { name: /HEX/ }).press('Escape')
    await popover.waitFor({ state: 'detached' })
    const focusReturned = await capsule.evaluate((element) => document.activeElement === element)
    await capsule.click()
    await popover.waitFor()
    const openScreenshot = join(output, `appearance-${scenario.name}-popover.png`)
    await page.screenshot({ path: openScreenshot, fullPage: true, animations: 'disabled' })
    await popover.evaluate(async (element) => {
      await Promise.all(
        element
          .getAnimations({ subtree: true })
          .filter((animation) => animation.effect?.getComputedTiming().iterations !== Infinity)
          .map((animation) => animation.finished.catch(() => {})),
      )
    })
    const measurement = await page.evaluate(() => {
      const bounds = (element) => {
        const rect = element.getBoundingClientRect()
        return {
          x: rect.x,
          y: rect.y,
          width: rect.width,
          height: rect.height,
          right: rect.right,
          bottom: rect.bottom,
        }
      }
      const group = document.querySelector('[role=radiogroup]')
      const thumbnails = [...group.querySelectorAll('label > span[aria-hidden=true]')]
      const colorPlane = document.querySelector('[data-slot=popover-content] [role=slider]')
      const capsule = document.querySelector(
        '[data-config-card=interface-appearance] button[aria-haspopup=dialog]',
      )
      const scrollRegions = [
        document.documentElement,
        document.body,
        document.querySelector('#root'),
        document.querySelector('[data-page-template]'),
        document.querySelector('[data-slot=popover-content]'),
      ]
        .filter(Boolean)
        .map((element) => ({
          tag: element.tagName,
          slot:
            element.getAttribute('data-slot') ||
            element.getAttribute('data-page-template') ||
            element.id,
          scrollWidth: element.scrollWidth,
          clientWidth: element.clientWidth,
          horizontalOverflow: element.scrollWidth > element.clientWidth + 1,
        }))
      const offenders = [...document.querySelectorAll('body *')]
        .filter((element) => {
          const style = getComputedStyle(element)
          const rect = element.getBoundingClientRect()
          return (
            style.position !== 'absolute' &&
            style.position !== 'fixed' &&
            rect.width > 0 &&
            rect.height > 0 &&
            (rect.right > innerWidth + 1 || rect.left < -1)
          )
        })
        .map((element) => ({
          tag: element.tagName,
          slot: element.getAttribute('data-slot'),
          text: element.textContent?.slice(0, 80),
          rect: bounds(element),
        }))
        .slice(0, 15)
      const rootStyle = getComputedStyle(document.documentElement)
      const uiStyle = getComputedStyle(document.body)
      const popover = document.querySelector('[data-slot=popover-content]')
      return {
        viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
        theme: document.documentElement.dataset.theme,
        darkClass: document.documentElement.classList.contains('dark'),
        typography: {
          rootPx: parseFloat(rootStyle.fontSize),
          uiPx: parseFloat(uiStyle.fontSize),
          uiBaseToken: rootStyle.getPropertyValue('--font-ui-base').trim(),
          bodyFontFamily: uiStyle.fontFamily,
        },
        colors: {
          body: uiStyle.backgroundColor,
          foreground: uiStyle.color,
          backgroundToken: rootStyle.getPropertyValue('--bg').trim(),
          popover: getComputedStyle(popover).backgroundColor,
        },
        modeGroup: {
          ...bounds(group),
          gridTemplateColumns: getComputedStyle(group).gridTemplateColumns,
          columnGap: getComputedStyle(group).columnGap,
        },
        thumbnails: thumbnails.map((element) => ({
          ...bounds(element),
          aspectRatio:
            element.getBoundingClientRect().width / element.getBoundingClientRect().height,
        })),
        capsule: bounds(capsule),
        colorPlane: bounds(colorPlane),
        popover: bounds(popover),
        scrollRegions,
        offenders,
        labels: [...group.querySelectorAll('label')].map((element) => ({
          text: element.innerText,
          ...bounds(element),
          scrollWidth: element.scrollWidth,
          clientWidth: element.clientWidth,
        })),
      }
    })
    const checks = {
      themeApplied:
        measurement.theme === scenario.theme &&
        measurement.darkClass === (scenario.theme === 'dark'),
      threeColumns:
        measurement.thumbnails.length === 3 &&
        new Set(measurement.thumbnails.map(({ y }) => y)).size === 1,
      noHorizontalOverflow:
        measurement.scrollRegions.every(({ horizontalOverflow }) => !horizontalOverflow) &&
        measurement.offenders.length === 0,
      popoverWithinViewport:
        measurement.popover.x >= 0 && measurement.popover.right <= scenario.width,
      focusOnOpen,
      tabToHueAndHex: focusSequence[1].type === 'range' && focusSequence[2].type === 'text',
      focusReturned,
    }
    metrics.scenarios.push({
      ...scenario,
      screenshots: { closed: closedScreenshot, popover: openScreenshot },
      measurement,
      focusSequence,
      checks,
    })
    for (const [check, passed] of Object.entries(checks))
      if (!passed) metrics.findings.push({ scenario: scenario.name, check })
    await page.getByRole('textbox', { name: /HEX/ }).press('Escape')
    await popover.waitFor({ state: 'detached' })
  }
  await writeFile(join(output, 'metrics.json'), JSON.stringify(metrics, null, 2) + '\n')
  console.log(`VISUAL_ARTIFACTS ${output}`)
  console.log(
    `MEASUREMENTS ${JSON.stringify(metrics.scenarios.map(({ name, measurement, checks }) => ({ name, rootPx: measurement.typography.rootPx, uiPx: measurement.typography.uiPx, grid: measurement.modeGroup.gridTemplateColumns, thumbnails: measurement.thumbnails.map(({ width, height }) => ({ width, height })), capsule: { width: measurement.capsule.width, height: measurement.capsule.height }, colorPlane: { width: measurement.colorPlane.width, height: measurement.colorPlane.height }, checks })))}`,
  )
  if (metrics.findings.length) console.log(`LAYOUT_FINDINGS ${JSON.stringify(metrics.findings)}`)
  assert.deepEqual(metrics.externalRequests, [], 'The fixture must not contact external services')
  assert.deepEqual(errors, [])
} finally {
  await browser?.close()
  await new Promise((resolve) => server.close(resolve))
}
