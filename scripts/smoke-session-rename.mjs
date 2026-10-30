import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdtemp, rm } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'
import { startSmokeStyleServer } from './fixtures/smoke-style-server.mjs'
import { AgentRuntimeService } from '../runtime/runtime/agent-runtime.mjs'
import { ensureSessionFilePersisted } from '../runtime/runtime/session-file-persist.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
// 显式传入地址时沿用（例如对着正在运行的开发服务器复核）；不传时自带只提供样式与字体的服务，
// CI 与本机都不依赖外部开发服务器。
let origin = process.argv[2] ? new URL(process.argv[2]).origin : ''
assert(!origin || ['127.0.0.1', 'localhost', '[::1]'].includes(new URL(origin).hostname))
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
      import { QueryClientProvider, useQuery } from '@tanstack/react-query'
      import { ChatHistoryPage } from './src/features/chat/ChatHistoryPage.tsx'
      import { AppDialog } from './src/components/layout/AppDialog.tsx'
      import { useAppDialog } from './src/hooks/useAppDialog.ts'
      import {
        queryClient,
        installStartupQueryEvents,
        startupQueryOptions,
      } from './src/lib/startup-queries.ts'
      const uninstall = installStartupQueryEvents(window)
      window.addEventListener('pagehide', () => { uninstall(); queryClient.clear() }, { once: true })
      // 看板、泳道条与会话页头都读同一份会话目录缓存；这里用同一个查询列出名称与整理状态，
      // 核对历史页的改名、置顶、归档写回了共享缓存（旧侧栏 ProjectTaskList 已不挂载）。
      function CatalogMirror() {
        const { data } = useQuery(startupQueryOptions('sessions'))
        return <ul data-smoke-catalog>
          {(data?.sessions ?? []).map((session) => <li
            key={session.id}
            data-pinned={session.pinned ? 'true' : 'false'}
            data-archived={session.archived ? 'true' : 'false'}
          >{session.name}</li>)}
        </ul>
      }
      function Fixture() {
        const dialog = useAppDialog()
        const [notice, setNotice] = useState(null)
        const navigate = () => {}
        const notify = (message, tone) => setNotice({ message, tone })
        const props = { navigate, notify, requestText: dialog.prompt, requestConfirm: dialog.confirm }
        return <QueryClientProvider client={queryClient}>
          <div data-smoke-layout>
            <aside data-smoke-catalog-view><CatalogMirror /></aside>
            <main data-smoke-history>
              <ChatHistoryPage {...props} query="" />
            </main>
            <div data-smoke-notices>
              {notice && <div role={notice.tone === 'error' ? 'alert' : 'status'}>{notice.message}</div>}
            </div>
          </div>
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

// 历史页是新外壳里常驻的会话管理入口（重命名、置顶、归档）；会话页头“⋯ › 重命名会话”由看板冒烟在
// 真实会话页上验收。目录镜像只读共享缓存，业务组件、对话框和刷新事件均保持原样。
const html = `<!doctype html><html lang="zh-CN"><head>
<meta name="viewport" content="width=device-width,initial-scale=1">
<link rel="stylesheet" href="/src/index.css?direct">
<style>
[data-smoke-layout] { display:grid; grid-template-columns:236px minmax(0,1fr); gap:16px; width:100%; padding:16px; }
[data-smoke-catalog-view], [data-smoke-history] { min-width:0; }
[data-smoke-catalog-view] { padding:8px; background:var(--panel); }
[data-smoke-notices] { grid-column:1/-1; }
@media(max-width:650px) { [data-smoke-layout] { grid-template-columns:minmax(0,1fr); } }
</style></head><body><div id="root"></div>
<script type="module" src="/__session-rename-fixture.js"></script></body></html>`

const runtimes = []
const directories = []
let browser
let styleServer = null
try {
  if (!origin) {
    styleServer = await startSmokeStyleServer(root)
    origin = styleServer.origin
  }
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
    const organizationMutations = []
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
        url.pathname === `/api/sessions/${created.id}/change-summary` &&
        request.method() === 'GET'
      )
        return route.fulfill({ json: await runtime.getSessionChangeSummary(created.id) })
      if (
        url.origin === origin &&
        url.pathname === `/api/sessions/${created.id}/organization` &&
        request.method() === 'PATCH'
      ) {
        const patch = request.postDataJSON()
        const updated = await runtime.updateSessionOrganization(created.id, patch)
        organizationMutations.push(patch)
        return route.fulfill({
          status: updated ? 200 : 404,
          json: updated || { error: 'Not found' },
        })
      }
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

    const catalog = page.locator('[data-smoke-catalog]')
    const history = page.locator('[data-smoke-history]')
    const dialog = page.getByRole('dialog', { name: '重命名会话', exact: true })
    const assertSynchronized = async (name) => {
      try {
        await page.waitForFunction(
          (expected) =>
            Array.from(document.querySelectorAll('[data-smoke-catalog] li')).some(
              (item) => item.textContent === expected,
            ) && document.querySelector('.chat-history-copy strong')?.textContent === expected,
          name,
          { timeout: 3_000 },
        )
      } catch (error) {
        const actual = {
          catalog: await catalog.innerText(),
          history: await history.innerText(),
        }
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
      // 历史页“更多操作”菜单：分别用指针点击与键盘（聚焦后按 Enter）打开。
      const more = history.getByRole('button', { name: `${name} 的更多操作`, exact: true })
      if (entry === 'keyboard') {
        await more.focus()
        await page.keyboard.press('Enter')
      } else await more.click()
      await page.getByRole('menuitem', { name: '重命名会话', exact: true }).click()
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
      await assertSynchronized(initialName)
      let currentName = initialName
      // 手动标题只改第 20 字后的内容，防止自动标题的截断规则误用于手动重命名。
      const sharedPrefix = 'Repeat title prefix!'
      assert.equal(Array.from(sharedPrefix).length, 20)
      const entries = ['keyboard', 'keyboard', 'history', 'history']
      for (const [index, entry] of entries.entries()) {
        const before = mutations.length
        await openRename(entry, currentName)
        await dialog.getByRole('textbox').fill('   ')
        assert(await dialog.getByRole('button', { name: '保存', exact: true }).isDisabled())
        await dialog.getByRole('button', { name: '取消', exact: true }).click()
        await dialog.waitFor({ state: 'detached' })
        assert.equal(mutations.length, before)

        await openRename(entry, currentName)
        const nextName = `${sharedPrefix} ${entry} ${index + 1}`
        await dialog.getByRole('textbox').fill(`  ${nextName}  `)
        await page.screenshot({
          path: join(screenshots, `${entry}-${index + 1}-dialog-${width}.png`),
          fullPage: true,
        })
        const updated = await saveRename(`  ${nextName}  `, 200)
        assert.equal(updated.name, nextName)
        assert.equal(mutations.length, before + 1)
        assert.equal(mutations.at(-1).name, nextName)
        await assertSynchronized(nextName)
        await page.screenshot({
          path: join(screenshots, `${entry}-${index + 1}-saved-${width}.png`),
          fullPage: true,
        })
        await runtime.sessionMetaWrite
        assert.equal(runtime.openStoredSession(sessionFile).getSessionName(), nextName)
        await page.reload()
        await assertSynchronized(nextName)
        await page.screenshot({
          path: join(screenshots, `${entry}-${index + 1}-reloaded-${width}.png`),
          fullPage: true,
        })
        currentName = nextName

        // 使用真实运行状态触发 Runtime 拒绝，分别覆盖通知回调与历史页错误区。
        runtime.liveSessions.set(created.id, { streaming: true })
        await openRename(entry, currentName)
        const failure = await saveRename(`Rejected ${entry} ${width}`, 409)
        assert.match(failure.error, /正在运行/)
        const errorArea = history
        await errorArea.getByRole('alert').filter({ hasText: failure.error }).waitFor()
        await assertSynchronized(currentName)
        runtime.liveSessions.delete(created.id)
        await page.screenshot({
          path: join(screenshots, `${entry}-${index + 1}-error-${width}.png`),
          fullPage: true,
        })
        await page.reload()
        await assertSynchronized(currentName)
      }
      // 整理状态必须跨共享目录、历史页和重载同步；归档只改变列表可见性，不删除会话。
      const openActions = () =>
        history.getByRole('button', { name: `${currentName} 的更多操作` }).click()
      await openActions()
      await page.getByRole('menuitem', { name: '置顶', exact: true }).click()
      await catalog.locator('li[data-pinned="true"]').filter({ hasText: currentName }).waitFor()
      assert.equal(
        (await runtime.listSessions()).find((item) => item.id === created.id)?.pinned,
        true,
      )
      await page.reload()
      await assertSynchronized(currentName)
      await catalog.locator('li[data-pinned="true"]').filter({ hasText: currentName }).waitFor()
      await openActions()
      await page.getByRole('menuitem', { name: '归档', exact: true }).click()
      await history.getByRole('heading', { name: '暂无未归档会话' }).waitFor()
      await catalog.locator('li[data-archived="true"]').filter({ hasText: currentName }).waitFor()
      assert.equal(
        (await runtime.listSessions()).find((item) => item.id === created.id)?.archived,
        true,
      )
      await history.getByRole('button', { name: '已归档（1）' }).click()
      await history.getByRole('button', { name: `打开会话 ${currentName}` }).waitFor()
      await page.reload()
      await history.getByRole('button', { name: '已归档（1）' }).click()
      await openActions()
      await page.getByRole('menuitem', { name: '恢复', exact: true }).click()
      assert.equal(
        (await runtime.listSessions()).find((item) => item.id === created.id)?.archived,
        false,
      )
      await history.getByRole('button', { name: '未归档' }).click()
      await assertSynchronized(currentName)
      assert.deepEqual(organizationMutations, [
        { pinned: true },
        { archived: true },
        { archived: false },
      ])
      assert.deepEqual(errors, [])
      assert.deepEqual(unexpectedRequests, [])
      assert.deepEqual(
        mutations.map((item) => item.status),
        entries.flatMap(() => [200, 409]),
      )
      console.log(
        `PASS ${width}px: repeated history-page rename (menu and keyboard), persisted pin/archive/restore, shared catalog sync and Runtime errors`,
      )
    } catch (error) {
      await page
        .screenshot({ path: join(screenshots, `failure-${width}.png`), fullPage: true })
        .catch(() => {})
      console.error(
        JSON.stringify(
          { width, mutations, organizationMutations, errors, unexpectedRequests },
          null,
          2,
        ),
      )
      throw error
    } finally {
      await context.close()
    }
  }
} finally {
  try {
    try {
      await browser?.close()
    } finally {
      await styleServer?.close()
    }
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
