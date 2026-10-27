// 移动壳回归：真实 React 组件、隔离接口/桥接和本机浏览器，不启动模型或读取用户配置。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { readFile, readdir } from 'node:fs/promises'
import { createServer } from 'node:http'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const mocks = {
  '@/components/MarkdownMessage': 'export default function MarkdownMessage() { return null }',
  '@/app/use-i18n': `export function useI18n() { return {t: key => key, language: 'en-US'} }`,
  '@/features/chat/chat-api': `
    const data = () => ({files:[{path:'sample.txt',pending:true,approved:false,reverted:false,canRevert:true,added:1,removed:0}],summary:{files:window.evidence.revision,pending:1,added:1,removed:0}})
    export const chatApi = {
      getSessionFileChanges:async () => { window.evidence.reads++; return data() },
      approveSessionFileChanges:async () => { window.evidence.approvals++; return data() },
      revertSessionFileChanges:async () => { window.evidence.reverts++; return data() },
    }`,
  '@/features/chat/session-change-summary-api': `export async function invalidateSessionChangeSummary() {}`,
  '@/lib/api': `export async function apiJson(url, options) {
    if (url === '/api/sessions') return {sessions: []}
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
const mockPaths = new Map(
  Object.keys(mocks).map((name) => [join(root, 'src', name.slice(2)), name]),
)
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
        builder.onResolve({ filter: /.*/ }, (args) => {
          // 当前 Node 分支的领域模块使用同目录导入；别名与相对路径应命中同一隔离边界。
          const mock = mocks[args.path]
            ? args.path
            : args.path.startsWith('.')
              ? mockPaths.get(resolve(args.resolveDir, args.path))
              : undefined
          return mock ? { path: mock, namespace: 'mock' } : null
        })
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
  const projectPage = await browser.newPage({ viewport: { width: 1200, height: 900 } })
  projectPage.setDefaultTimeout(5000)
  await projectPage.goto(`http://127.0.0.1:${server.address().port}/?sidebar`)
  const newProject = projectPage.getByRole('button', {
    name: 'navigation:appSidebar.newProject',
    exact: true,
  })
  await newProject.waitFor()
  await projectPage.evaluate(() => {
    window.projectPickCalls = 0
    window.projectPickResult = null
    window.pisperDesktop.pickDirectory = () => {
      window.projectPickCalls++
      return Promise.resolve(window.projectPickResult)
    }
  })
  await newProject.click()
  await projectPage.waitForFunction(() => window.projectPickCalls === 1)
  assert.equal(
    await projectPage.getByRole('dialog').count(),
    0,
    'Native cancellation must not open a manual directory dialog',
  )
  assert.deepEqual(await projectPage.evaluate(() => window.sidebarEvidence.navigations), [])
  await projectPage.evaluate(() => {
    window.projectPickResult = '/workspaces/native-project'
  })
  await newProject.click()
  await projectPage.waitForFunction(() => window.sidebarEvidence.navigations.includes('chat'))
  await projectPage.evaluate(() => {
    window.pisperDesktop.pickDirectory = () => Promise.reject(new Error('native_directory_failed'))
  })
  await newProject.click()
  await projectPage.waitForFunction(() =>
    window.sidebarEvidence.notices.some(
      (notice) => notice.message === 'native_directory_failed' && notice.tone === 'error',
    ),
  )
  await projectPage.evaluate(() => {
    delete window.pisperDesktop.pickDirectory
  })
  await newProject.click()
  await projectPage
    .getByRole('textbox', { name: 'common:workspacePicker.workingDirectoryPath', exact: true })
    .waitFor()
  await projectPage.close()

  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, hasTouch: true })
  page.setDefaultTimeout(10000)
  const errors = []
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(`http://127.0.0.1:${server.address().port}`)
  await page.waitForFunction(() => window.controls)
  await page.evaluate(() => window.controls.setWorkspaceOpen(true))
  const workspacePath = page.getByRole('textbox', {
    name: 'common:workspacePicker.workingDirectoryPath',
    exact: true,
  })
  await workspacePath.waitFor()
  assert.equal(await workspacePath.inputValue(), '/workspaces/initial')
  await workspacePath.fill('   /workspaces/manual   ')
  await page.evaluate(() => {
    window.failWorkspaceSave = true
  })
  await page
    .getByRole('button', { name: 'common:workspacePicker.useThisFolder', exact: true })
    .click()
  await page.getByRole('alert').filter({ hasText: 'workspace_save_failed' }).waitFor()
  assert.equal(await workspacePath.inputValue(), '   /workspaces/manual   ')
  await page.evaluate(() => {
    window.failWorkspaceSave = false
  })
  await workspacePath.press('Enter')
  await page.getByRole('dialog').waitFor({ state: 'hidden' })
  assert.deepEqual(await page.evaluate(() => window.pickerEvidence.selected), [
    '/workspaces/manual',
  ])

  // Android 本机继续通过系统 SAF 导入，远程模式只接受 Runtime 路径。
  await page.evaluate(() => {
    window.__PISPER_MOBILE_APP__ = true
    window.__PISPER_MOBILE_PLATFORM__ = 'android'
    window.nativeWorkspaceMode = 'remote'
    window.importedWorkspace = null
    window.__TAURI_INTERNALS__ = {
      invoke: (command) => {
        if (command === 'mobile_state')
          return Promise.resolve({ mode: window.nativeWorkspaceMode, onDevice: { running: true } })
        if (command === 'mobile_import_workspace_directory')
          return Promise.resolve({ path: window.importedWorkspace })
        return Promise.reject(new Error('unexpected_native_command'))
      },
    }
    window.controls.setWorkspaceOpen(true)
  })
  await workspacePath.waitFor()
  assert.equal(
    await page
      .getByRole('button', { name: 'common:workspacePicker.importFolderCopy', exact: true })
      .count(),
    0,
  )
  await page.getByRole('button', { name: 'common:ui.cancel', exact: true }).click()
  await page.evaluate(() => {
    window.nativeWorkspaceMode = 'local'
    window.controls.setWorkspaceOpen(true)
  })
  const importFolder = page.getByRole('button', {
    name: 'common:workspacePicker.importFolderCopy',
    exact: true,
  })
  await importFolder.waitFor()
  await importFolder.click()
  await page.waitForFunction(
    () =>
      ![...document.querySelectorAll('button')].find(
        (node) => node.textContent === 'common:workspacePicker.importFolderCopy',
      ).disabled,
  )
  assert.equal(
    await workspacePath.inputValue(),
    '/workspaces/initial',
    'Cancellation preserves the current path',
  )
  await page.evaluate(() => {
    window.importedWorkspace = '/workspace/import-1/Project'
  })
  await importFolder.click()
  await page.waitForFunction(
    () =>
      document.querySelector('input:not([type="file"])')?.value === '/workspace/import-1/Project',
  )
  await workspacePath.press('Enter')
  await page.getByRole('dialog').waitFor({ state: 'hidden' })
  assert.deepEqual(await page.evaluate(() => window.pickerEvidence.selected), [
    '/workspaces/manual',
    '/workspace/import-1/Project',
  ])
  await page.evaluate(() => {
    delete window.__PISPER_MOBILE_APP__
    delete window.__PISPER_MOBILE_PLATFORM__
    delete window.__TAURI_INTERNALS__
  })

  // Web 和手机交给浏览器系统选择器；不能回退到服务器文件树。
  const attachment = page.getByRole('button', {
    name: 'chat:focusSession.addAttachment',
    exact: true,
  })
  for (const client of ['web', 'mobile-app']) {
    await page.evaluate((client) => window.controls.setClient(client), client)
    const chooserPromise = page.waitForEvent('filechooser')
    await attachment.click()
    const chooser = await chooserPromise
    await chooser.setFiles({
      name: client + '.txt',
      mimeType: 'text/plain',
      buffer: Buffer.from('fixture'),
    })
    await page.waitForFunction(
      (name) => window.pickerEvidence.uploads.includes(name),
      client + '.txt',
    )
  }
  await page.evaluate(() => {
    window.controls.setClient('web')
    window.pisperDesktop.pickFiles = () => Promise.resolve(['/workspaces/picked.txt'])
  })
  await attachment.click()
  await page.waitForFunction(() => window.pickerEvidence.attachments.length === 1)
  assert.deepEqual(await page.evaluate(() => window.pickerEvidence.attachments[0]), {
    id: 'path:/workspaces/picked.txt',
    kind: 'path',
    name: 'picked.txt',
    path: '/workspaces/picked.txt',
  })
  await page.evaluate(() => {
    window.pisperDesktop.pickFiles = () => Promise.resolve([])
  })
  await attachment.click()
  assert.equal(await page.evaluate(() => window.pickerEvidence.attachments.length), 1)
  await page.evaluate(() => {
    window.pisperDesktop.pickFiles = () => Promise.reject(new Error('native_picker_failed'))
  })
  await attachment.click()
  await page.waitForFunction(() => window.pickerEvidence.errors.includes('native_picker_failed'))

  await page.getByRole('button', { name: 'newTerminal', exact: true }).click()
  await page.getByRole('button', { name: 'terminal:workspace.create', exact: true }).click()
  await page.waitForFunction(() => window.evidence.created === 1)
  const terminal = page.locator('[data-terminal-id]')
  await terminal.waitFor()
  assert.equal(await page.evaluate(() => window.evidence.terminalOptions[0].cwd), '/workspaces/one')
  await page.getByRole('tab', { name: 'navigation:mobileShell.sessionTools', exact: true }).click()
  await page
    .getByRole('textbox', { name: 'session tool draft', exact: true })
    .fill('keep auxiliary draft')
  await page.getByRole('tab', { name: 'navigation:mobileShell.terminal', exact: true }).click()
  await page.getByRole('tab', { name: 'navigation:mobileShell.sessionTools', exact: true }).click()
  assert.equal(
    await page.getByRole('textbox', { name: 'session tool draft', exact: true }).inputValue(),
    'keep auxiliary draft',
  )
  await page.getByRole('tab', { name: 'navigation:mobileShell.terminal', exact: true }).click()
  await page.evaluate(() => window.controls.setPane('chat'))
  await page.waitForFunction(
    () => document.querySelector('section[aria-label="navigation:mobileShell.context"]').inert,
  )
  await page.evaluate(() => window.controls.setPane('context'))
  await terminal.waitFor()
  await page.evaluate(() => window.controls.setTab('extensions'))
  const toggle = page.getByRole('switch').first()
  await toggle.waitFor()
  await toggle.click()
  await page.getByText('plugins:pluginsPage.unsaved', { exact: true }).waitFor()
  await page.evaluate(() => window.controls.setTab('terminal'))
  await terminal.waitFor()
  assert.equal(
    await page.evaluate(() => window.evidence.closed),
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

  assert.equal(
    await page.getByRole('tab', { name: 'navigation:mobileShell.files', exact: true }).count(),
    0,
  )

  await page.setViewportSize({ width: 1024, height: 768 })
  await page.evaluate(() => window.controls.setMode('pad'))
  await page.waitForFunction(() => document.querySelector('[data-mobile-shell="pad"]'))
  const tabletPager = page.getByRole('tablist', { name: 'navigation:mobileShell.pager' })
  await tabletPager.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).click()
  const widths = await page
    .locator('[data-mobile-shell] > div > section')
    .evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().width))
  assert.equal(widths.length, 3)
  assert.deepEqual(widths, [0, 1024, 0], 'Tablet shows only the selected pane at full width')
  const tabletNav = await tabletPager.boundingBox()
  const tabletShell = await page.locator('[data-mobile-shell="pad"]').boundingBox()
  assert.ok(Math.abs(tabletNav.y + tabletNav.height - tabletShell.y - tabletShell.height) < 1)
  await page.getByRole('textbox', { name: 'composer', exact: true }).fill('tablet draft')
  const tabletCdp = await page.context().newCDPSession(page)
  for (const [type, touchPoints] of [
    ['touchStart', [{ x: 600, y: 300 }]],
    ['touchMove', [{ x: 450, y: 300 }]],
    ['touchEnd', []],
  ])
    await tabletCdp.send('Input.dispatchTouchEvent', { type, touchPoints })
  await page.waitForFunction(() => !document.getElementById('mobile-shell-pane-context').inert)
  await page.setViewportSize({ width: 768, height: 1024 })
  assert.equal(await page.locator('#mobile-shell-pane-chat').evaluate((node) => node.inert), true)
  await tabletPager.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).click()
  assert.equal(
    await page.getByRole('textbox', { name: 'composer', exact: true }).inputValue(),
    'tablet draft',
  )
  await page.setViewportSize({ width: 390, height: 844 })
  await page.evaluate(() => {
    window.controls.setMode('phone')
    window.controls.setPane('chat')
  })
  await page.waitForFunction(() => document.querySelector('[data-mobile-shell="phone"]'))
  await page.waitForFunction(
    () =>
      Math.abs(
        document
          .querySelector('section[aria-label="navigation:mobileShell.chat"]')
          .getBoundingClientRect().x,
      ) < 1,
  )
  const cdp = await page.context().newCDPSession(page)
  const swipe = async (start, end) => {
    for (const [type, points] of [
      ['touchStart', [start]],
      ['touchMove', [end]],
      ['touchEnd', []],
    ])
      await cdp.send('Input.dispatchTouchEvent', { type, touchPoints: points })
    await page.evaluate(() => new Promise(requestAnimationFrame))
  }
  await swipe({ x: 270, y: 300 }, { x: 260, y: 150 })
  assert.equal(
    await page
      .locator('section[aria-label="navigation:mobileShell.context"]')
      .evaluate((node) => node.inert),
    true,
    'Vertical conversation scrolling must not switch panes',
  )
  const composer = await page.getByRole('textbox', { name: 'composer' }).boundingBox()
  assert(composer)
  await swipe(
    { x: composer.x + composer.width - 10, y: composer.y + 10 },
    { x: composer.x + 10, y: composer.y + 10 },
  )
  assert.equal(
    await page
      .locator('section[aria-label="navigation:mobileShell.context"]')
      .evaluate((node) => node.inert),
    true,
    'Editing the composer must not switch panes',
  )
  const pager = page.getByRole('tablist', { name: 'navigation:mobileShell.pager' })
  await page.evaluate(() => {
    document.documentElement.dataset.mobileKeyboard = 'open'
  })
  assert.equal(await pager.isVisible(), false, 'The pager yields height to the software keyboard')
  await page.evaluate(() => {
    delete document.documentElement.dataset.mobileKeyboard
  })
  assert.equal(await pager.isVisible(), true)
  const pagerTargets = await pager
    .getByRole('tab')
    .evaluateAll((nodes) => nodes.map((node) => node.getBoundingClientRect().height))
  assert.ok(pagerTargets.every((height) => height >= 44))
  const pagerBounds = await pager.boundingBox()
  const shellBounds = await page.locator('[data-mobile-shell="phone"]').boundingBox()
  assert.ok(
    Math.abs(pagerBounds.y + pagerBounds.height - shellBounds.y - shellBounds.height) < 1,
    'Phone pane navigation must stay at the bottom of the shell',
  )
  await pager.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).focus()
  await pager.getByRole('tab', { name: 'navigation:mobileShell.chat', exact: true }).press('End')
  await page.waitForFunction(
    () => !document.querySelector('section[aria-label="navigation:mobileShell.context"]').inert,
  )
  await pager
    .getByRole('tab', { name: 'navigation:mobileShell.context', exact: true })
    .press('Home')
  await page.waitForFunction(
    () => !document.querySelector('section[aria-label="navigation:mobileShell.sessions"]').inert,
  )
  await pager
    .getByRole('tab', { name: 'navigation:mobileShell.sessions', exact: true })
    .press('ArrowRight')
  await page.waitForFunction(() => {
    const pane = document.querySelector('section[aria-label="navigation:mobileShell.chat"]')
    return !pane.inert && Math.abs(pane.getBoundingClientRect().x) < 1
  })
  assert.equal(await page.evaluate(() => document.activeElement?.id), 'mobile-shell-tab-chat')
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
  assert.equal(await page.evaluate(() => window.evidence.closed), 0)
  await page.evaluate(() => window.controls.setMounted(false))
  await page.waitForFunction(() => window.evidence.closed === 1)
  assert.deepEqual(errors, [])
  console.log(
    'PASS mobile shell: system file selection, workspace paths, terminal lifetime, independent save, run safety, layout and touch navigation',
  )
} finally {
  await browser?.close()
  await new Promise((resolve, reject) =>
    server.close((error) => (error ? reject(error) : resolve())),
  )
}
