import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'
import { AgentRuntimeService } from '../runtime/runtime/agent-runtime.mjs'
import { ensureSessionFilePersisted } from '../runtime/runtime/session-file-persist.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const origin = new URL(process.argv[2] || 'http://127.0.0.1:5173').origin
assert(['127.0.0.1', 'localhost', '[::1]'].includes(new URL(origin).hostname))
const executablePath =
  process.env.PISPER_SMOKE_BROWSER ||
  [
    chromium.executablePath(),
    join(
      homedir(),
      'Library/Caches/ms-playwright/chromium-1234/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing',
    ),
  ].find(existsSync)
assert(executablePath && existsSync(executablePath), 'Set PISPER_SMOKE_BROWSER to Chromium.')
const screenshots = await mkdtemp(join(tmpdir(), 'pisper-session-rename-smoke-'))
console.log(`Screenshots: ${screenshots}`)

const fixture = await build({
  stdin: {
    contents: `
      import React, { useState } from 'react'
      import { createRoot } from 'react-dom/client'
      import { QueryClientProvider } from '@tanstack/react-query'
      import { SidebarRecentSessions } from './src/components/layout/SidebarRecentSessions.tsx'
      import { ChatHistoryPage } from './src/features/chat/ChatHistoryPage.tsx'
      import { SidebarProvider } from './src/components/ui/sidebar.tsx'
      import { AppDialog } from './src/components/layout/AppDialog.tsx'
      import { useAppDialog } from './src/hooks/useAppDialog.ts'
      import { queryClient, installStartupQueryEvents } from './src/lib/startup-queries.ts'
      const uninstall = installStartupQueryEvents(window)
      window.addEventListener('pagehide', () => { uninstall(); queryClient.clear() }, { once: true })
      function Fixture() {
        const dialog = useAppDialog()
        const [view, setView] = useState(location.hash.slice(1))
        const [notice, setNotice] = useState(null)
        const navigate = page => { location.hash = page; setView(page) }
        const notify = (message, tone) => setNotice({ message, tone })
        const props = { navigate, notify, requestText: dialog.prompt, requestConfirm: dialog.confirm }
        return <QueryClientProvider client={queryClient}>
          <SidebarProvider>
            <div data-smoke-layout>
              <aside className="nav-list" data-smoke-sidebar>
                <SidebarRecentSessions {...props} />
              </aside>
              <main data-smoke-history data-view={view}>
                {view === 'chatHistory' && <ChatHistoryPage {...props} query="" />}
              </main>
              <div data-smoke-notices>
                {notice && <div role={notice.tone === 'error' ? 'alert' : 'status'}>{notice.message}</div>}
              </div>
            </div>
          </SidebarProvider>
          <AppDialog dialog={dialog.dialog} onClose={dialog.close} onFinish={dialog.finish} />
        </QueryClientProvider>
      }
      createRoot(document.getElementById('root')).render(<Fixture />)
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

// 窄屏直接展示侧栏区块，避免抽屉遮挡历史页；业务组件、对话框和刷新事件均保持原样。
const html = `<!doctype html><html lang="zh-CN"><head>
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="stylesheet" href="/src/index.css?direct">
<style>
[data-smoke-layout] { display:grid; grid-template-columns:236px minmax(0,1fr); gap:16px; width:100%; padding:16px; }
[data-smoke-sidebar], [data-smoke-history] { min-width:0; }
[data-smoke-sidebar] { padding:8px; background:var(--panel); }
[data-smoke-notices] { grid-column:1/-1; }
@media(max-width:650px) { [data-smoke-layout] { grid-template-columns:minmax(0,1fr); } }
</style></head><body><div id="root"></div>
<script type="module" src="/__session-rename-fixture.js"></script></body></html>`

const runtimes = []
const directories = []
let browser
try {
  browser = await chromium.launch({ executablePath, headless: true })
  for (const width of [1440, 390]) {
    const directory = await mkdtemp(join(tmpdir(), 'pisper-session-rename-data-'))
    directories.push(directory)
    const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
    runtimes.push(runtime)
    runtime.settingsManager = {
      getGlobalSettings: () => ({
        defaultProvider: 'openai',
        defaultModel: 'gpt-5.4',
        defaultThinkingLevel: 'medium',
      }),
    }
    const initialName = `Rename smoke ${width}`
    const created = await runtime.createSession(initialName, directory)
    const pending = runtime.pendingSessions.get(created.id)
    assert(pending)
    await ensureSessionFilePersisted(pending.manager, initialName, directory)
    const sessionFile = pending.manager.getSessionFile()
    runtime.pendingSessions.clear()
    assert.equal(runtime.sessions.size, 0)
    // 必须在重命名前预热真实磁盘目录缓存，否则无法捕捉列表返回旧标题的回归。
    assert.equal(
      (await runtime.listSessions()).find((item) => item.id === created.id)?.name,
      initialName,
    )
    assert(runtime.storedSessionsCache?.some((item) => item.id === created.id))

    const context = await browser.newContext({
      viewport: { width, height: 900 },
      locale: 'zh-CN',
      serviceWorkers: 'block',
    })
    const page = await context.newPage()
    page.setDefaultTimeout(5_000)
    const errors = []
    const unexpectedRequests = []
    const mutations = []
    page.on('pageerror', (error) => errors.push(error.message))
    // 拦截所有请求，未声明的 API 一律失败，绝不落到正在运行的用户 Runtime。
    await page.route('**/*', async (route) => {
      const request = route.request()
      const url = new URL(request.url())
      if (url.origin === origin && url.pathname === '/__session-rename-smoke__')
        return route.fulfill({ contentType: 'text/html', body: html })
      if (url.origin === origin && url.pathname === '/__session-rename-fixture.js')
        return route.fulfill({ contentType: 'text/javascript', body: fixture.outputFiles[0].text })
      if (url.origin === origin && url.pathname === '/api/sessions' && request.method() === 'GET')
        return route.fulfill({ json: { sessions: await runtime.listSessions() } })
      if (
        url.origin === origin &&
        url.pathname === `/api/sessions/${created.id}` &&
        request.method() === 'PATCH'
      ) {
        const { name } = request.postDataJSON()
        try {
          const updated = await runtime.renameSession(created.id, name)
          mutations.push({ name, status: updated ? 200 : 404 })
          return route.fulfill({
            status: updated ? 200 : 404,
            json: updated || { error: 'Session not found' },
          })
        } catch (error) {
          mutations.push({ name, status: 409 })
          return route.fulfill({ status: 409, json: { error: error.message } })
        }
      }
      if (
        url.origin === origin &&
        request.method() === 'GET' &&
        (url.pathname === '/src/index.css' ||
          url.pathname.startsWith('/node_modules/@fontsource-variable/'))
      )
        return route.continue()
      if (url.origin === origin && url.pathname === '/favicon.ico')
        return route.fulfill({ status: 204 })
      unexpectedRequests.push(`${request.method()} ${url.pathname}`)
      return route.abort('blockedbyclient')
    })

    const sidebar = page.locator('[data-smoke-sidebar]')
    const history = page.locator('[data-smoke-history]')
    const dialog = page.getByRole('dialog', { name: '重命名会话', exact: true })
    const assertSynchronized = async (name) => {
      try {
        await page.waitForFunction(
          (expected) =>
            document.querySelector('.nav-history-item > span')?.textContent === expected &&
            document.querySelector('.chat-history-copy strong')?.textContent === expected,
          name,
          { timeout: 3_000 },
        )
      } catch (error) {
        const actual = await page.evaluate(() => ({
          sidebar: document.querySelector('.nav-history-item > span')?.textContent,
          history: document.querySelector('.chat-history-copy strong')?.textContent,
        }))
        throw new Error(
          `Rename did not synchronize: ${JSON.stringify({ expected: name, ...actual })}`,
          { cause: error },
        )
      }
      assert.equal(
        (await runtime.listSessions()).find((item) => item.id === created.id)?.name,
        name,
      )
    }
    const openRename = async (entry, name) => {
      if (entry === 'sidebar') {
        await sidebar.getByRole('button', { name, exact: true }).click({ button: 'right' })
        await page.getByRole('menuitem', { name: '重命名会话', exact: true }).click()
      } else {
        await history.getByRole('button', { name: '重命名会话', exact: true }).click()
      }
      await dialog.waitFor()
      assert.equal(await dialog.getByRole('textbox').inputValue(), name)
      const bounds = await dialog.boundingBox()
      assert(bounds && bounds.x >= 0 && bounds.x + bounds.width <= width)
      assert.equal(await dialog.evaluate((element) => getComputedStyle(element).position), 'fixed')
    }
    const saveRename = async (name, status) => {
      await dialog.getByRole('textbox').fill(name)
      const responsePromise = page.waitForResponse(
        (response) =>
          response.url() === `${origin}/api/sessions/${created.id}` &&
          response.request().method() === 'PATCH',
      )
      await dialog.getByRole('button', { name: '保存', exact: true }).click()
      const response = await responsePromise
      assert.equal(response.status(), status)
      await dialog.waitFor({ state: 'detached' })
      return response.json()
    }

    try {
      await page.goto(`${origin}/__session-rename-smoke__`)
      await sidebar.locator('.nav-history-view-all').click()
      await assertSynchronized(initialName)
      let currentName = initialName
      for (const entry of ['sidebar', 'history']) {
        const before = mutations.length
        await openRename(entry, currentName)
        await dialog.getByRole('textbox').fill('   ')
        assert(await dialog.getByRole('button', { name: '保存', exact: true }).isDisabled())
        await dialog.getByRole('button', { name: '取消', exact: true }).click()
        await dialog.waitFor({ state: 'detached' })
        assert.equal(mutations.length, before)

        await openRename(entry, currentName)
        const nextName = `${entry} renamed ${width}`
        await dialog.getByRole('textbox').fill(`  ${nextName}  `)
        await page.screenshot({
          path: join(screenshots, `${entry}-dialog-${width}.png`),
          fullPage: true,
        })
        const updated = await saveRename(`  ${nextName}  `, 200)
        assert.equal(updated.name, nextName)
        assert.equal(mutations.length, before + 1)
        assert.equal(mutations.at(-1).name, nextName)
        await assertSynchronized(nextName)
        await page.screenshot({
          path: join(screenshots, `${entry}-saved-${width}.png`),
          fullPage: true,
        })
        await runtime.sessionMetaWrite
        assert.equal(runtime.openStoredSession(sessionFile).getSessionName(), nextName)
        await page.reload()
        await assertSynchronized(nextName)
        await page.screenshot({
          path: join(screenshots, `${entry}-reloaded-${width}.png`),
          fullPage: true,
        })
        currentName = nextName

        // 使用真实运行状态触发 Runtime 拒绝，分别覆盖通知回调与历史页错误区。
        runtime.liveSessions.set(created.id, { streaming: true })
        await openRename(entry, currentName)
        const failure = await saveRename(`Rejected ${entry} ${width}`, 409)
        assert.match(failure.error, /正在运行/)
        const errorArea = entry === 'sidebar' ? page.locator('[data-smoke-notices]') : history
        await errorArea.getByRole('alert').filter({ hasText: failure.error }).waitFor()
        await assertSynchronized(currentName)
        runtime.liveSessions.delete(created.id)
        await page.screenshot({
          path: join(screenshots, `${entry}-error-${width}.png`),
          fullPage: true,
        })
        await page.reload()
        await assertSynchronized(currentName)
      }
      assert.deepEqual(errors, [])
      assert.deepEqual(unexpectedRequests, [])
      assert.deepEqual(
        mutations.map((item) => item.status),
        [200, 409, 200, 409],
      )
      console.log(
        `PASS ${width}px: real dialog, sidebar/history rename, shared refresh, reload, disk title, cancel/empty, Runtime errors`,
      )
    } catch (error) {
      await page
        .screenshot({ path: join(screenshots, `failure-${width}.png`), fullPage: true })
        .catch(() => {})
      console.error(JSON.stringify({ width, mutations, errors, unexpectedRequests }, null, 2))
      throw error
    } finally {
      await context.close()
    }
  }
} finally {
  try {
    await browser?.close()
  } finally {
    const writes = await Promise.allSettled(runtimes.map((runtime) => runtime.sessionMetaWrite))
    await Promise.all(
      directories.map((directory) => rm(directory, { recursive: true, force: true })),
    )
    assert(
      writes.every((write) => write.status === 'fulfilled'),
      'Session metadata write failed',
    )
  }
}
