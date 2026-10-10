import assert from 'node:assert/strict'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'

// 所有页面请求由夹具拦截；不启动真实后端，也不使用开发者的浏览器配置。
const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = join(root, 'release/project-directory-smoke')
const origin = 'http://127.0.0.1:45873'
const selectedPath = 'C:\\Pisper-Smoke-New-Project'
await mkdir(output, { recursive: true })
await build({
  stdin: {
    contents: `
      import React from 'react'
      import { createRoot } from 'react-dom/client'
      import { QueryClientProvider } from '@tanstack/react-query'
      import { pickSystemDirectory } from './src/lib/platform/pick-system-directory.ts'
      import { AttachmentPicker } from './src/features/chat/components/composer/AttachmentPicker.tsx'
      import { SidebarRecentSessions } from './src/components/layout/SidebarRecentSessions.tsx'
      import { SidebarProvider } from './src/components/ui/sidebar.tsx'
      import { queryClient } from './src/lib/startup/startup-queries.ts'
      const state = window.__projectSmoke
      state.pickSystemDirectory = pickSystemDirectory
      window.addEventListener('pisper:session-create-requested', (event) => state.created.push(event.detail))
      window.addEventListener('pagehide', () => queryClient.clear(), { once: true })
      const reactRoot = createRoot(document.getElementById('root'))
      state.unmount = () => { reactRoot.unmount(); queryClient.clear() }
      reactRoot.render(
        <QueryClientProvider client={queryClient}>
          <SidebarProvider mobile={false}>
            <AttachmentPicker cwd="/fixture" selection={{ attachments: [], addFiles: async files => { state.uploaded = files.map(file => file.name); return true }, addAttachments: files => { state.attachments = files }, setError: error => state.notices.push({message:String(error),tone:'error'}) }} />
            <SidebarRecentSessions
              navigate={(value) => state.navigation.push(value)}
              requestText={async () => null}
              requestConfirm={async () => false}
              notify={(message, tone) => state.notices.push({ message, tone })}
            />
          </SidebarProvider>
        </QueryClientProvider>
      )
    `,
    resolveDir: root,
    loader: 'tsx',
  },
  outfile: join(output, 'fixture.js'),
  bundle: true,
  format: 'esm',
  platform: 'browser',
  target: 'chrome120',
  alias: { '@': join(root, 'src'), '@shared': join(root, 'shared') },
  define: { 'process.env.NODE_ENV': '"production"' },
  loader: { '.css': 'empty' },
  logLevel: 'warning',
})
const fixture = await readFile(join(output, 'fixture.js'), 'utf8')
const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<style>body{margin:20px;font:14px sans-serif}#root{width:340px}#sidebar-recent-sessions{min-height:260px}button{min-height:28px}svg{width:16px;height:16px}[role=dialog]{position:fixed;inset:60px 20px auto;background:white;border:1px solid #999;padding:20px}[role=menu]{background:white;border:1px solid #999;padding:8px}[role=menuitem]{padding:8px}</style>
</head><body><div id="root"></div><script type="module" src="/fixture.js"></script></body></html>`
await writeFile(join(output, 'fixture.html'), html)
const report = { checks: [], failures: [], pageErrors: [], deniedRequests: [] }
const executablePath = process.env.PISPER_SMOKE_BROWSER
const browser = await chromium.launch({
  ...(executablePath
    ? { executablePath: resolve(executablePath) }
    : { executablePath: chromium.executablePath() }),
  headless: true,
})
let activePage

async function setup(native = true) {
  const context = await browser.newContext({
    viewport: { width: 1100, height: 800 },
    serviceWorkers: 'block',
  })
  await context.addInitScript(
    ({ native }) => {
      window.__projectSmoke = {
        calls: [],
        created: [],
        navigation: [],
        notices: [],
        directoryRequests: 0,
        directoryOutcome: 'success',
        uploaded: [],
        attachments: [],
      }
      if (native) {
        window.pisperDesktop = {
          pickDirectory: (initialDirectory) => {
            window.__projectSmoke.calls.push(initialDirectory ?? null)
            return new Promise((resolve, reject) => {
              window.__projectSmoke.resolve = resolve
              window.__projectSmoke.reject = reject
            })
          },
        }
      }
    },
    { native },
  )
  const page = await context.newPage()
  activePage = page
  page.setDefaultTimeout(3500)
  page.on('pageerror', (error) => report.pageErrors.push(error.message))
  await page.route('**/*', async (route) => {
    const request = route.request()
    const url = new URL(request.url())
    if (url.origin === origin && url.pathname === '/')
      return route.fulfill({ contentType: 'text/html', body: html })
    if (url.origin === origin && url.pathname === '/fixture.js')
      return route.fulfill({ contentType: 'text/javascript', body: fixture })
    if (url.origin === origin && url.pathname === '/api/sessions' && request.method() === 'GET')
      return route.fulfill({ json: { sessions: [] } })
    if (
      url.origin === origin &&
      url.pathname === '/api/desktop/pick-directory' &&
      request.method() === 'POST'
    ) {
      await page.evaluate(() => window.__projectSmoke.directoryRequests++)
      assert.deepEqual(request.postDataJSON(), { initialDirectory: '' })
      const outcome = await page.evaluate(() => window.__projectSmoke.directoryOutcome)
      if (outcome === 'error')
        return route.fulfill({
          status: 503,
          json: { code: 'system_picker_unavailable', error: 'picker-fixture-error' },
        })
      return route.fulfill({
        json:
          outcome === 'invalid'
            ? { path: 123 }
            : { path: outcome === 'cancel' ? null : selectedPath },
      })
    }
    if (url.origin === origin && url.pathname === '/favicon.ico')
      return route.fulfill({ status: 204 })
    report.deniedRequests.push(`${request.method()} ${url.origin}${url.pathname}`)
    return route.abort('blockedbyclient')
  })
  await page.goto(origin)
  await page.getByRole('button', { name: '新建项目', exact: true }).waitFor()
  return { page, close: () => context.close() }
}

async function openEntry(page, entry) {
  if (entry === 'toolbar') {
    await page.getByRole('button', { name: '新建项目', exact: true }).click()
  } else {
    await page
      .locator('#sidebar-recent-sessions')
      .click({ button: 'right', position: { x: 30, y: 180 } })
    await page.getByRole('menuitem', { name: '新建项目', exact: true }).click()
  }
  await page.waitForFunction(
    () => window.__projectSmoke.calls.length > 0 || window.__projectSmoke.directoryRequests > 0,
  )
}

async function assertNativeOnly(page, entry) {
  const snapshot = await page.evaluate(() => ({
    calls: window.__projectSmoke.calls.length,
    directoryRequests: window.__projectSmoke.directoryRequests,
    dialogs: document.querySelectorAll('[role="dialog"]').length,
  }))
  assert.equal(
    snapshot.calls,
    1,
    `${entry}: expected native picker; observed ${JSON.stringify(snapshot)}`,
  )
  assert.equal(snapshot.dialogs, 0, `${entry}: custom picker appeared despite native support`)
  assert.equal(
    snapshot.directoryRequests,
    0,
    `${entry}: native flow browsed directories through HTTP`,
  )
}

try {
  for (const entry of ['toolbar', 'context-menu']) {
    const { page, close } = await setup()
    await openEntry(page, entry)
    await assertNativeOnly(page, entry)
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.created), [])
    await page.evaluate((path) => window.__projectSmoke.resolve(path), selectedPath)
    await page.waitForFunction(() => window.__projectSmoke.created.length === 1)
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.created), [
      { cwd: selectedPath },
    ])
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.navigation), ['chat'])
    report.checks.push(`${entry}: native selection creates one chat request without custom picker`)
    await close()
  }

  for (const outcome of ['resolve', 'reject']) {
    const { page, close } = await setup()
    await openEntry(page, 'toolbar')
    await assertNativeOnly(page, `unmount-${outcome}`)
    const lateResult = await page.evaluate(
      async ({ outcome, path }) => {
        const state = window.__projectSmoke
        state.unmount()
        if (outcome === 'resolve') state.resolve(path)
        else state.reject(new Error('late-native-picker-fixture-error'))
        // 经过一次渲染边界，让已决 Promise 的微任务全部完成后再检查副作用。
        await new Promise((resolve) => requestAnimationFrame(resolve))
        return {
          renderedChildren: document.getElementById('root').childElementCount,
          created: state.created,
          navigation: state.navigation,
          notices: state.notices,
        }
      },
      { outcome, path: selectedPath },
    )
    assert.deepEqual(lateResult, { renderedChildren: 0, created: [], navigation: [], notices: [] })
    report.checks.push(`unmount: late ${outcome} creates no chat, navigation, or notice`)
    await close()
  }

  {
    const { page, close } = await setup()
    // 同一事件批次内重复点击，覆盖 React 状态提交前的重入窗口。
    await page.getByRole('button', { name: '新建项目', exact: true }).evaluate((button) => {
      button.click()
      button.click()
    })
    await assertNativeOnly(page, 'duplicate-click')
    await page.evaluate(() => window.__projectSmoke.resolve(null))
    await page.waitForFunction(
      () => !document.querySelector('button[aria-label="新建项目"]').disabled,
    )
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.created), [])
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.navigation), [])
    await page.getByRole('button', { name: '新建项目', exact: true }).click()
    await page.waitForFunction(() => window.__projectSmoke.calls.length === 2)
    await page.evaluate(() => window.__projectSmoke.resolve(null))
    report.checks.push(
      'duplicate clicks launch once; cancellation creates nothing and permits retry',
    )
    await close()
  }

  {
    const { page, close } = await setup()
    await openEntry(page, 'toolbar')
    await page.evaluate(() =>
      window.__projectSmoke.reject(new Error('native-picker-fixture-error')),
    )
    await page.waitForFunction(() => window.__projectSmoke.notices.length === 1)
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.notices), [
      { message: 'native-picker-fixture-error', tone: 'error' },
    ])
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.created), [])
    assert.equal(await page.getByRole('dialog').count(), 0)
    report.checks.push('native error is notified without chat creation or custom fallback')
    await close()
  }

  for (const entry of ['toolbar', 'context-menu']) {
    const { page, close } = await setup(false)
    await openEntry(page, entry)
    assert.equal(await page.getByRole('dialog').count(), 0)
    await page.waitForFunction(() => window.__projectSmoke.directoryRequests === 1)
    await page.waitForFunction(() => window.__projectSmoke.created.length === 1)
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.created), [
      { cwd: selectedPath },
    ])
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.calls), [])
    report.checks.push(
      `${entry}: browser invokes the system directory endpoint without custom browsing`,
    )
    await close()
  }
  for (const outcome of ['cancel', 'error', 'invalid']) {
    const { page, close } = await setup(false)
    await page.evaluate((outcome) => {
      window.__projectSmoke.directoryOutcome = outcome
    }, outcome)
    await openEntry(page, 'toolbar')
    await page.waitForFunction(
      () => !document.querySelector('button[aria-label="新建项目"]').disabled,
    )
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.created), [])
    assert.equal(await page.getByRole('dialog').count(), 0)
    assert.equal(
      await page.evaluate(() => window.__projectSmoke.notices.length),
      outcome === 'cancel' ? 0 : 1,
    )
    report.checks.push(`browser ${outcome}: no chat creation or custom fallback`)
    await close()
  }
  {
    const { page, close } = await setup(false)
    const chooser = page.waitForEvent('filechooser')
    await page.locator('button.attach-trigger').click()
    await (
      await chooser
    ).setFiles({ name: 'fixture.txt', mimeType: 'text/plain', buffer: Buffer.from('fixture') })
    await page.waitForFunction(() => window.__projectSmoke.uploaded.length === 1)
    assert.deepEqual(await page.evaluate(() => window.__projectSmoke.uploaded), ['fixture.txt'])
    assert.equal(await page.getByRole('dialog').count(), 0)
    report.checks.push('browser attachments use the system file input and upload contents')
    await close()
  }
  {
    const { page, close } = await setup()
    await page.evaluate(() => {
      window.pisperDesktop.pickFiles = (cwd) => {
        window.__projectSmoke.fileCwd = cwd
        return Promise.resolve(['/fixture/a.txt'])
      }
    })
    await page.locator('button.attach-trigger').click()
    await page.waitForFunction(() => window.__projectSmoke.attachments.length === 1)
    assert.equal(await page.evaluate(() => window.__projectSmoke.fileCwd), '/fixture')
    assert.equal(
      await page.evaluate(() => window.__projectSmoke.attachments[0].path),
      '/fixture/a.txt',
    )
    assert.equal(await page.getByRole('dialog').count(), 0)
    report.checks.push('desktop attachments use native file selection and retain path references')
    await close()
  }
  {
    const { page, close } = await setup(false)
    const result = await page.evaluate(async () => {
      window.__PISPER_MOBILE_APP__ = true
      window.__PISPER_MOBILE_PLATFORM__ = 'android'
      const calls = []
      let mode = 'local'
      window.__TAURI__ = {
        core: {
          invoke: (command) => {
            calls.push(command)
            return Promise.resolve(
              command === 'mobile_state'
                ? { mode, onDevice: { running: true } }
                : { path: '/mobile/imported' },
            )
          },
        },
      }
      const path = await window.__projectSmoke.pickSystemDirectory()
      mode = 'remote'
      let denied = false
      try {
        await window.__projectSmoke.pickSystemDirectory()
      } catch {
        denied = true
      }
      return { path, denied, calls }
    })
    assert.deepEqual(result, {
      path: '/mobile/imported',
      denied: true,
      calls: ['mobile_state', 'mobile_import_workspace_directory', 'mobile_state'],
    })
    report.checks.push(
      'Android local selection invokes system import; remote mode never imports phone paths',
    )
    await close()
  }
  assert.deepEqual(report.pageErrors, [])
  assert.deepEqual(report.deniedRequests, [])
  console.log(`PASS: ${report.checks.length} project-directory interaction checks`)
} catch (error) {
  report.failures.push(error.stack || String(error))
  if (activePage && !activePage.isClosed()) {
    await activePage.screenshot({ path: join(output, 'failure.png') }).catch(() => {})
  }
  console.error(error.message)
  process.exitCode = 1
} finally {
  await browser.close()
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  console.log(`Report: ${join(output, 'report.json')}`)
}
