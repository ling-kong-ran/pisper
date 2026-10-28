import assert from 'node:assert/strict'
import { readdir, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import { chromium } from 'playwright-core'

const output = await mkdtemp(join(tmpdir(), 'pisper-workbench-'))
const sessions = ['a', 'b', 'c', 'd'].map((id) => ({
  id,
  name: `Task ${id.toUpperCase()}`,
  cwd: `/fixture/project-${id === 'd' ? 'a' : id}`,
  modified: '2026-10-14T00:00:00Z',
  model: 'fixture/model',
  streaming: false,
}))
const requests = []
let savedLayout = null
let invalidLayout = false
let runningTask = false
let snapshotVersion = 0
let holdProjectA = false
let releaseProjectA = () => {}
const css = (await readdir('dist/assets')).find((name) => /^index-.*\.css$/.test(name))
assert.ok(css, 'build production CSS before running workbench verification')
const server = await createServer({
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
  plugins: [
    {
      name: 'workbench-fixture',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          const url = new URL(req.url, 'http://fixture')
          if (url.pathname === '/__preview') {
            res.setHeader('Content-Type', 'text/html')
            res.end('<p>Legacy preview fixture</p>')
            return
          }
          if (url.pathname === '/__workbench' || url.pathname === '/__review') {
            const html = await server.transformIndexHtml(
              '/__workbench',
              `<!doctype html><html lang="zh-CN"><head><meta name="viewport" content="width=device-width,initial-scale=1"/><link rel="stylesheet" href="/dist/assets/${css}"/></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/${url.pathname === '/__review' ? 'frontend-review' : 'frontend-workbench'}.tsx"></script></body></html>`,
            )
            res.setHeader('Content-Type', 'text/html')
            res.end(html)
            return
          }
          if (!url.pathname.startsWith('/api/')) return next()
          requests.push({ method: req.method, path: url.pathname })
          if (holdProjectA && url.pathname === '/api/sessions/a/vcs/changes')
            await new Promise((resolve) => {
              releaseProjectA = resolve
            })
          let data = {}
          if (url.pathname === '/api/config')
            data = {
              provider: 'fixture',
              model: 'model',
              providers: [
                {
                  id: 'fixture',
                  name: 'Fixture',
                  configured: true,
                  enabled: true,
                  models: [{ id: 'model', kind: 'chat' }],
                },
              ],
            }
          else if (url.pathname === '/api/sessions')
            data = {
              sessions: sessions.map((session) => ({
                ...session,
                streaming: runningTask && session.id === 'a',
              })),
            }
          else if (url.pathname === '/api/client-info') data = { client: 'web' }
          else if (url.pathname === '/api/settings/chat-dock-layout') {
            if (req.method === 'PUT') {
              let body = ''
              for await (const chunk of req) body += chunk
              savedLayout = JSON.parse(body)
            }
            data = invalidLayout ? { version: 999, engine: 'future', layout: {} } : savedLayout
          } else if (url.pathname.endsWith('/tree'))
            data = {
              sessionId: url.pathname.split('/')[3],
              leafId: null,
              nodeCount: 0,
              branchCount: 0,
              streaming: false,
              nodes: [],
              lineage: null,
            }
          else if (url.pathname.endsWith('/messages'))
            data = {
              messages: Array.from({ length: 32 }, (_, index) => ({
                id: `message-${index}`,
                ...(index === 0
                  ? {
                      attachments: [
                        {
                          id: `asset-${url.pathname.split('/')[3]}`,
                          name: 'design.md',
                          mimeType: 'text/markdown',
                        },
                        { id: 'broken-image', name: 'broken.png', mimeType: 'image/png' },
                      ],
                    }
                  : {}),
                role: index % 2 ? 'agent' : 'user',
                text:
                  `Task ${url.pathname.split('/')[3]} message ${index}\n\n` +
                  'A reading position must survive task switching. '.repeat(30),
              })),
              hasOlder: false,
              start: 0,
              total: 32,
            }
          else if (url.pathname.endsWith('/live'))
            data = {
              streaming: runningTask && url.pathname.includes('/a/'),
              messages: [{ id: 'running-message', role: 'agent', text: 'Running task output' }],
              model: 'fixture/model',
              startedAt: '2026-10-14T00:00:00Z',
              tools: [],
              activityFeed: [],
              queuedInputs: [],
            }
          else if (url.pathname.endsWith('/thinking-level'))
            data = { thinkingLevel: 'medium', availableThinkingLevels: ['medium'] }
          else if (url.pathname === '/api/skills')
            data = {
              skills: [],
              packages: [{ source: 'npm:only-package', scope: 'global', installed: false }],
            }
          else if (url.pathname === '/api/sessions/snapshot/file-changes')
            data = {
              files: [
                {
                  path: 'snapshot.txt',
                  status: 'modified',
                  added: 1,
                  removed: 1,
                  changeCount: snapshotVersion + 1,
                  changedAt: String(snapshotVersion),
                  snapshot: true,
                  canRevert: true,
                  approved: false,
                  reverted: false,
                  pending: true,
                },
              ],
              summary: { files: 1, pending: 1, added: 1, removed: 1 },
            }
          else if (url.pathname === '/api/sessions/snapshot/file-changes/diff')
            data = {
              diff:
                'diff --git a/snapshot.txt b/snapshot.txt\n--- a/snapshot.txt\n+++ b/snapshot.txt\n@@ -1 +1 @@\n-before\n+' +
                (snapshotVersion ? 'new snapshot' : 'old snapshot') +
                '\n',
            }
          else if (url.pathname.endsWith('/file-changes'))
            data = { files: [], summary: { files: 0, pending: 0, added: 0, removed: 0 } }
          else if (url.pathname.endsWith('/vcs/changes'))
            data = {
              isRepo: true,
              files: [
                { path: url.pathname.includes('/b/') ? 'b-only.ts' : 'src/one.ts', status: 'M' },
                { path: 'src/two.ts', status: 'A' },
              ],
              diff: '',
            }
          else if (url.pathname.endsWith('/vcs/file-diff'))
            data = {
              isRepo: true,
              diff:
                'diff --git a/src/one.ts b/src/one.ts\n--- a/src/one.ts\n+++ b/src/one.ts\n@@ -1,100 +1,100 @@\n' +
                Array.from({ length: 100 }, (_, i) => '-old ' + i + '\n+new ' + i + '\n').join(''),
            }
          else if (url.pathname.endsWith('/content'))
            data = url.pathname.includes('broken-image')
              ? { kind: 'file', mimeType: 'image/png' }
              : { kind: 'text', text: 'Source belongs to ' + url.pathname.split('/')[3] }
          else if (url.pathname.endsWith('/download')) {
            res.setHeader('Content-Type', 'image/png')
            res.end('invalid raster fixture')
            return
          } else if (url.pathname.endsWith('/workflow-runs')) data = { runs: [] }
          else if (url.pathname.endsWith('/commands')) data = { commands: [] }
          else if (url.pathname === '/api/settings/browser-preferences') data = { preferences: {} }
          res.setHeader('Content-Type', 'application/json')
          res.end(JSON.stringify(data))
        })
      },
    },
  ],
})
let browser
const reviewFailures = []
const reviewExpect = (condition, label) => {
  if (!condition) reviewFailures.push(label)
}
try {
  await server.listen()
  const address = server.httpServer.address()
  browser = await chromium.launch({ headless: true })
  const backgroundPage = await browser.newPage({
    viewport: { width: 2400, height: 1000 },
    locale: 'zh-CN',
  })
  await backgroundPage.goto(`http://127.0.0.1:${address.port}/__workbench`)
  await backgroundPage.locator('[data-task-workspace="a"]').waitFor({ timeout: 45000 })
  await backgroundPage.evaluate(() => window.workbenchFixture.open('b', 'right'))
  const backgroundComposer = backgroundPage.locator(
    '[data-task-workspace="b"] .focus-composer textarea',
  )
  await backgroundComposer.fill('继续编辑 B')
  // 使用自动展示的同一入口，验证后台任务更新不能改变当前输入目标。
  await backgroundPage.evaluate(() => window.workbenchFixture.revealFiles('a'))
  await backgroundPage.locator('[data-task-workspace="a"] [role="tab"]').waitFor()
  await backgroundPage.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(await backgroundComposer.evaluate((el) => el === document.activeElement), true)
  assert.equal(
    await backgroundPage.locator('[data-task-workspace="b"][data-active="true"]').count(),
    1,
    'Background resource reveal preserves the active task',
  )
  const foregroundComposer = backgroundPage.locator(
    '[data-task-workspace="a"] .focus-composer textarea',
  )
  await foregroundComposer.click()
  await backgroundPage.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  assert.equal(await foregroundComposer.evaluate((el) => el === document.activeElement), true)
  await backgroundPage.close()
  savedLayout = null
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, locale: 'zh-CN' })
  const errors = []
  await page.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  page.on('pageerror', (error) => errors.push(error.message))
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text())
  })

  await page.goto(`http://127.0.0.1:${address.port}/__workbench`)
  await page.locator('[data-task-workspace="a"]').waitFor({ timeout: 45000 })
  const initialPane = page.locator('[data-task-workspace="a"]')
  await page.setViewportSize({ width: 320, height: 1000 })
  const mobileMore = page.getByRole('button', { name: '项目与任务设置', exact: true })
  await mobileMore.waitFor()
  await mobileMore.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => {}))),
  )
  const mobileMoreBounds = await mobileMore.boundingBox()
  assert.ok(mobileMoreBounds.width >= 44 && mobileMoreBounds.height >= 44)
  await page.setViewportSize({ width: 1600, height: 1000 })
  await initialPane.getByRole('button', { name: '项目与任务设置', exact: true }).click()
  await page
    .getByRole('menuitem', { name: '打开会话上下文', exact: true })
    .waitFor({ timeout: 5000 })
  await page.getByRole('menuitem', { name: '追忆', exact: true }).waitFor()
  assert.equal(await initialPane.getByTestId('theme-toggle').count(), 0)
  await page.getByRole('menuitem', { name: /^主题：/ }).waitFor()
  await page.getByRole('menuitem', { name: '追忆', exact: true }).click()
  const recallDialog = page.getByRole('dialog', { name: '追忆', exact: true })
  await recallDialog.waitFor()
  await page.waitForFunction(() =>
    document.querySelector('[role="dialog"]')?.contains(document.activeElement),
  )
  await recallDialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.waitForFunction(() => document.activeElement?.id === 'task-a-more')
  await initialPane.locator('.focus-composer textarea').fill('A 的未发送草稿')
  await initialPane.locator('.transcript').waitFor()
  await initialPane.getByRole('button', { name: '置顶摘要', exact: true }).click()
  const summary = initialPane.getByRole('region', { name: '置顶摘要', exact: true })
  await summary.waitFor()
  await page.screenshot({
    path: join(output, 'summary-desktop.png'),
  })
  await initialPane.locator('.focus-composer textarea').click()
  assert.equal(await summary.isVisible(), true, 'desktop pinned summary survives background clicks')
  await summary.getByRole('button', { name: '项目变更', exact: false }).click()
  const changesPane = initialPane.getByRole('region', { name: '项目变更', exact: true })
  await changesPane.getByRole('button', { name: 'src/one.ts M', exact: true }).click()
  await changesPane.locator('[data-pisper-diff-row-count]').waitFor()
  await changesPane.getByRole('button', { name: '全部展开', exact: true }).click()
  await page.waitForFunction(
    () =>
      document.querySelectorAll('[data-task-workspace="a"] [data-pisper-diff-row-count]').length ===
      2,
  )
  await changesPane.getByRole('button', { name: '全部收起', exact: true }).click()
  assert.equal(await changesPane.locator('[data-pisper-diff-row-count]').count(), 0)
  await changesPane.getByRole('button', { name: 'src/one.ts M', exact: true }).click()
  const diffScroll = changesPane.locator('[data-pisper-diff-row-count]').locator('..')
  await diffScroll.evaluate((el) => {
    el.scrollTop = 300
    el.scrollLeft = 120
    el.dispatchEvent(new Event('scroll', { bubbles: true }))
  })
  await initialPane.getByRole('button', { name: '置顶摘要', exact: true }).click()
  await summary.getByRole('button', { name: 'design.md', exact: true }).click()
  await initialPane.getByRole('tab', { name: '项目变更', exact: true }).click()
  await changesPane.locator('[data-pisper-diff-row-count]').waitFor()
  const sourcesTabId = await initialPane
    .getByRole('tab', { name: '来源', exact: true })
    .getAttribute('id')
  await changesPane
    .locator('[data-pisper-diff-row-count]')
    .locator('..')
    .evaluate((el, id) => {
      el.scrollTop = 450
      el.scrollLeft = 160
      document.getElementById(id).click()
    }, sourcesTabId)
  await initialPane.getByRole('tab', { name: '项目变更', exact: true }).click()
  await changesPane.locator('[data-pisper-diff-row-count]').waitFor()
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  const lastResourceOffset = await changesPane
    .locator('[data-pisper-diff-row-count]')
    .locator('..')
    .evaluate((el) => ({ top: el.scrollTop, left: el.scrollLeft }))
  reviewExpect(
    Math.abs(lastResourceOffset.top - 450) < 3 && Math.abs(lastResourceOffset.left - 160) < 3,
    'R5: resource tab must flush its last scroll before native scroll dispatch',
  )
  await initialPane.getByRole('tab', { name: '来源', exact: true }).click()

  const sourcesPane = initialPane.getByRole('region', { name: '来源', exact: true })
  await sourcesPane.getByRole('button', { name: '预览', exact: true }).click()
  await page.getByRole('dialog').getByText('Source belongs to asset-a', { exact: true }).waitFor()
  assert.equal(await page.evaluate(() => window.__pisperRequestBack()), true)
  await page.getByRole('dialog').waitFor({ state: 'hidden' })
  assert.equal(await initialPane.getByRole('tab', { name: '来源', exact: true }).isVisible(), true)
  await sourcesPane.getByRole('button', { name: 'broken.png', exact: true }).click()
  await sourcesPane.getByRole('button', { name: '预览', exact: true }).click()
  await page.getByRole('dialog').getByText('此文件暂不支持内容预览。', { exact: true }).waitFor()
  await page.getByRole('dialog').getByRole('button', { name: '关闭预览', exact: true }).click()
  // 来源页复用真实系统附件动作；取消、错误与浏览器选择由移动壳验收覆盖。
  await page.evaluate(() => {
    window.pisperDesktop = {
      ...window.pisperDesktop,
      pickFiles: (cwd) => {
        window.nativeAttachmentCwd = cwd
        return Promise.resolve(['/fixture/picked.txt'])
      },
    }
  })
  await sourcesPane.getByRole('button', { name: '添加附件', exact: true }).click()
  await initialPane.locator('.attachment-tray').getByText('picked.txt', { exact: true }).waitFor()
  assert.equal(await page.evaluate(() => window.nativeAttachmentCwd), '/fixture/project-a')
  assert.equal(
    await initialPane.locator('.attachment-tray').getByText('picked.txt', { exact: true }).count(),
    1,
  )
  await sourcesPane.getByRole('textbox', { name: '搜索来源', exact: true }).fill('missing')
  await sourcesPane.getByText('没有匹配的来源', { exact: true }).waitFor()
  await initialPane.getByRole('button', { name: '关闭辅助页面 · 来源', exact: true }).click()
  await initialPane.getByRole('button', { name: '关闭辅助页面 · 项目变更', exact: true }).click()

  await initialPane.locator('.transcript').evaluate((el) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, bubbles: true }))
    el.scrollTop = 360
    el.dispatchEvent(new Event('scroll', { bubbles: true }))
  })
  await initialPane.getByRole('button', { name: '项目与任务设置', exact: true }).click()
  await page.getByRole('menuitem', { name: '拆分到右侧', exact: true }).hover()
  await page.getByRole('menuitem', { name: 'Task B', exact: true }).click()
  await page.locator('[data-task-workspace="b"]').waitFor()
  assert.equal(await page.locator('[data-task-workspace]').count(), 2)
  await page.evaluate(() => window.workbenchFixture.open('c'))
  await page.locator('[data-task-workspace="c"]').waitFor()
  assert.equal(await page.locator('[data-task-workspace="a"]').count(), 1)
  assert.equal(await page.locator('[data-task-workspace="b"]').count(), 0)
  await page.evaluate(() => window.workbenchFixture.open('a'))
  assert.equal(await page.locator('[data-task-workspace]').count(), 2)
  const pane = page.locator('[data-task-workspace="a"]')
  await pane.getByRole('button', { name: '新建标签页', exact: true }).click()
  await pane.getByRole('heading', { name: '新建标签页', exact: true }).waitFor()
  const taskHeading = await pane.locator('[data-task-header]').boundingBox()
  const contentHeading = await pane.locator('.auxiliary-page-header').boundingBox()
  assert.ok(
    taskHeading && contentHeading && Math.abs(taskHeading.y - contentHeading.y) <= 1,
    'Task title and resource tabs share the top row in a split workspace',
  )
  assert.equal(await pane.getByRole('tab').count(), 1)
  await pane.getByRole('button', { name: '网页预览', exact: true }).click()
  const splitter = pane.getByRole('separator', { name: '调整会话上下文宽度', exact: true })
  const beforeWidth = Number(await splitter.getAttribute('aria-valuenow'))
  await splitter.press('ArrowLeft')
  assert.ok(Number(await splitter.getAttribute('aria-valuenow')) > beforeWidth)
  await splitter.press('ArrowRight')
  await splitter.dblclick()
  const beforeDrag = Number(await splitter.getAttribute('aria-valuenow'))
  const handle = await splitter.boundingBox()
  await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2)
  await page.mouse.down()
  await page.mouse.move(handle.x - 28, handle.y + handle.height / 2)
  await page.mouse.up()
  assert.ok(
    Number(await splitter.getAttribute('aria-valuenow')) > beforeDrag,
    'pointer resize changes only local content width',
  )
  await splitter.dblclick()
  const addressInput = pane.locator('form input')
  await addressInput.fill('https://example.invalid/kept-as-draft')
  const savedReadingPosition = await pane.locator('.transcript').evaluate((el) => {
    el.scrollTop = 720
    const top = el.scrollTop
    // 切换可以先于浏览器派发最后一次 scroll 事件；卸载不能丢失该位置。
    window.workbenchFixture.open('d')
    return top
  })
  assert.ok(savedReadingPosition > 0, 'the fixture must restore a real reading position')
  await page.locator('[data-task-workspace="a"]').waitFor({ state: 'detached' })
  assert.equal(
    await page.evaluate(() => window.workbenchFixture.state().a.conversationScrollTop),
    savedReadingPosition,
  )
  await page.evaluate(() => window.workbenchFixture.open('a'))
  await page.locator('[data-task-workspace="a"] form input').waitFor()
  assert.equal(
    await page.locator('[data-task-workspace="a"] form input').inputValue(),
    'https://example.invalid/kept-as-draft',
  )
  assert.equal(await pane.locator('.focus-composer textarea').inputValue(), 'A 的未发送草稿')
  await page.waitForFunction(
    (top) =>
      Math.abs(document.querySelector('[data-task-workspace="a"] .transcript').scrollTop - top) < 4,
    savedReadingPosition,
  )
  await page.setViewportSize({ width: 390, height: 844 })
  await page.locator('.mobile-session-shell').waitFor()
  const mobilePane = page.locator('[data-task-workspace="a"]')
  assert.equal(
    await mobilePane.locator('.focus-composer textarea').isVisible(),
    false,
    'content-only view must hide the conversation',
  )
  assert.equal(
    await mobilePane.getByRole('button', { name: '新建标签页', exact: true }).isVisible(),
    true,
    'task header stays visible in content mode',
  )
  await mobilePane.getByRole('button', { name: '置顶摘要', exact: true }).click()
  const sheet = page.getByRole('dialog', { name: '置顶摘要', exact: true })
  await sheet.waitFor()
  await page.screenshot({
    path: join(output, 'summary-mobile.png'),
    animations: 'disabled',
  })
  const sheetBox = await sheet.boundingBox()
  assert.ok(
    sheetBox.x >= 44 && sheetBox.y >= 0 && sheetBox.y + sheetBox.height <= 845,
    'summary sheet must fit inside the usable viewport',
  )
  await sheet.getByRole('button', { name: '关闭摘要', exact: true }).focus()
  for (let index = 0; index < 10; index++) {
    await page.keyboard.press('Tab')
    assert.equal(
      await sheet.evaluate((el) => el.contains(document.activeElement)),
      true,
      'mobile summary traps keyboard focus',
    )
  }
  assert.equal(
    await page.locator('body').evaluate((el) => getComputedStyle(el).pointerEvents),
    'none',
    'background cannot receive pointer input while summary is modal',
  )
  await sheet.press('Escape')
  await page.waitForFunction(() => document.activeElement?.id === 'task-a-summary-toggle')
  assert.equal(await page.evaluate(() => window.__pisperRequestBack()), true)
  assert.equal(
    await page.evaluate(() => window.workbenchFixture.state().a.pages.length),
    1,
    'back preserves tabs',
  )
  await mobilePane.locator('.focus-composer textarea').waitFor({ state: 'visible' })
  assert.equal(await mobilePane.locator('.focus-composer textarea').inputValue(), 'A 的未发送草稿')
  await page.evaluate(() => window.workbenchFixture.open('b'))
  await page.locator('[data-task-workspace="b"]').waitFor()
  await page.setViewportSize({ width: 1600, height: 1000 })
  await page.locator('.mobile-session-shell').waitFor({ state: 'detached' })
  await page.locator('[data-task-workspace="c"]').waitFor()
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  reviewExpect(
    (await page.locator('[data-task-workspace="b"][data-active="true"]').count()) === 1,
    'R1: mobile task selection must survive desktop restoration',
  )
  await page.evaluate(() => window.workbenchFixture.open('a'))
  await page.locator('[data-task-workspace="a"]').waitFor()

  await page.locator('[data-task-workspace="c"]').waitFor()
  assert.equal(await page.locator('[data-task-workspace]').count(), 2)
  assert.equal(
    await page.locator('[data-task-workspace="a"] .focus-composer textarea').inputValue(),
    'A 的未发送草稿',
  )

  for (const width of [650, 660, 900, 390, 1600, 650, 660, 900, 390, 1600]) {
    await page.setViewportSize({ width, height: 1000 })
    await page.waitForTimeout(100)
  }
  const restored = page.locator('[data-task-workspace="a"]')
  await restored.getByRole('button', { name: '项目与任务设置', exact: true }).click()
  await page.getByRole('menuitem', { name: '打开会话上下文', exact: true }).click()
  await restored.getByRole('button', { name: /关闭辅助页面/ }).click()
  assert.equal(await restored.getByRole('tab').count(), 0)
  await page.waitForFunction(() => document.activeElement?.id === 'task-a-new-content-tab')
  await restored.getByRole('button', { name: '项目与任务设置', exact: true }).click()
  await page.getByRole('menuitem', { name: '关闭分屏', exact: true }).click()
  await page.locator('[data-task-workspace="a"]').waitFor({ state: 'detached' })
  assert.equal(await page.locator('[data-task-workspace="c"]').count(), 1)
  await page.evaluate(() => window.workbenchFixture.open('a', 'right'))
  await page.locator('[data-task-workspace="a"]').waitFor()
  assert.equal(
    await page.locator('[data-task-workspace="a"] .focus-composer textarea').inputValue(),
    'A 的未发送草稿',
  )
  assert.equal(
    requests.some((request) => request.path.endsWith('/abort')),
    false,
  )
  const terminalOwner = page.locator('[data-task-workspace="a"]')
  await terminalOwner.getByRole('button', { name: '新建标签页', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '终端', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '创建终端', exact: true }).click()
  await terminalOwner.locator('[data-terminal-id]').waitFor()
  const terminalId = await terminalOwner
    .locator('[data-terminal-id]')
    .getAttribute('data-terminal-id')
  assert.equal(
    await page.evaluate(() => window.terminalFixture.created[0].cwd),
    '/fixture/project-a',
  )
  await page.evaluate(
    (id) => window.terminalFixture.emit(id, 'before closing view\r\n'),
    terminalId,
  )
  await terminalOwner
    .locator('.xterm-rows')
    .getByText('before closing view', { exact: true })
    .waitFor()
  await terminalOwner.getByRole('button', { name: '关闭辅助页面 · 终端', exact: true }).click()
  assert.deepEqual(await page.evaluate(() => window.terminalFixture.closed), [])
  await page.evaluate(
    (id) =>
      window.terminalFixture.emit(
        id,
        'output while view is closed\r\n' +
          Array.from({ length: 70 }, (_, i) => 'log ' + i + '\r\n').join(''),
      ),
    terminalId,
  )
  await terminalOwner.getByRole('button', { name: '新建标签页', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '后台进程', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '展开或收起日志', exact: true }).click()
  await terminalOwner.getByRole('checkbox', { name: '自动跟随输出', exact: true }).uncheck()
  await terminalOwner
    .getByRole('combobox', { name: '筛选进程状态', exact: true })
    .selectOption('running')
  await terminalOwner.getByRole('button', { name: '打开终端', exact: true }).click()
  await terminalOwner.locator('[data-terminal-id]').waitFor()
  assert.equal(
    await terminalOwner.locator('[data-terminal-id]').getAttribute('data-terminal-id'),
    terminalId,
  )
  await terminalOwner.getByRole('tab', { name: '后台进程', exact: true }).click()
  assert.equal(
    await terminalOwner.getByRole('combobox', { name: '筛选进程状态', exact: true }).inputValue(),
    'running',
  )
  assert.equal(
    await terminalOwner.getByRole('checkbox', { name: '自动跟随输出', exact: true }).isChecked(),
    false,
  )
  assert.match(await terminalOwner.locator('pre').innerText(), /output while view is closed/)
  await terminalOwner.getByRole('tab', { name: '终端', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '停止进程', exact: true }).click()
  assert.deepEqual(await page.evaluate(() => window.terminalFixture.closed), [terminalId])
  const secondOwner = page.locator('[data-task-workspace="c"]')
  await secondOwner.getByRole('button', { name: '新建标签页', exact: true }).click()
  await secondOwner.getByRole('button', { name: '终端', exact: true }).click()
  await secondOwner.getByRole('button', { name: '创建终端', exact: true }).click()
  await secondOwner.locator('[data-terminal-id]').waitFor()
  const secondTerminalId = await secondOwner
    .locator('[data-terminal-id]')
    .getAttribute('data-terminal-id')
  assert.notEqual(secondTerminalId, terminalId)
  assert.equal(
    await page.evaluate(() => window.terminalFixture.created[1].cwd),
    '/fixture/project-c',
  )
  await secondOwner.getByRole('button', { name: '项目与任务设置', exact: true }).click()
  await page.getByRole('menuitem', { name: '关闭分屏', exact: true }).click()
  await secondOwner.waitFor({ state: 'detached' })
  assert.deepEqual(await page.evaluate(() => window.terminalFixture.closed), [terminalId])
  await page.evaluate(
    (id) => window.terminalFixture.emit(id, 'continued in task C\r\n'),
    secondTerminalId,
  )
  await page.evaluate(() => window.workbenchFixture.open('c', 'right'))
  await secondOwner.locator('[data-terminal-id]').waitFor()
  assert.equal(
    await secondOwner.locator('[data-terminal-id]').getAttribute('data-terminal-id'),
    secondTerminalId,
  )
  await secondOwner
    .locator('.xterm-rows')
    .getByText('continued in task C', { exact: true })
    .waitFor()
  await secondOwner.getByRole('button', { name: '停止进程', exact: true }).click()
  assert.deepEqual(await page.evaluate(() => window.terminalFixture.closed), [
    terminalId,
    secondTerminalId,
  ])

  await page.evaluate(() => window.workbenchFixture.open('d'))
  const sharedA = page.locator('[data-task-workspace="a"]')
  const sharedD = page.locator('[data-task-workspace="d"]')
  for (const owner of [sharedA, sharedD]) {
    await owner.getByRole('button', { name: '置顶摘要', exact: true }).click()
    await owner
      .getByRole('region', { name: '置顶摘要', exact: true })
      .getByRole('button', { name: '项目变更', exact: false })
      .click()
    await owner
      .getByRole('region', { name: '项目变更', exact: true })
      .getByText('/fixture/project-a', { exact: true })
      .waitFor()
  }
  const separatorA = sharedA.getByRole('separator')
  const separatorD = sharedD.getByRole('separator')
  await page.setViewportSize({ width: 2600, height: 1000 })
  await separatorA.press('Home')
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  const otherWidth = await separatorD.getAttribute('aria-valuenow')
  await separatorA.press('ArrowLeft')
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  reviewExpect(
    (await separatorD.getAttribute('aria-valuenow')) === otherWidth,
    'R4: resizing A must not resize D',
  )
  await page.setViewportSize({ width: 1600, height: 1000 })
  await sharedA.getByRole('button', { name: 'src/one.ts M', exact: true }).click()
  assert.equal(
    await sharedD
      .getByRole('button', { name: 'src/one.ts M', exact: true })
      .getAttribute('aria-expanded'),
    'false',
    'shared project data does not share per-task expansion state',
  )
  await page.waitForTimeout(250)
  await Promise.all([
    page.waitForResponse(
      (response) =>
        response.request().method() === 'PUT' &&
        response.url().endsWith('/api/settings/chat-dock-layout'),
    ),
    page.evaluate(() => window.workbenchFixture.preview(location.origin + '/__preview')),
  ])
  await page.frameLocator('iframe').getByText('Legacy preview fixture', { exact: true }).waitFor()
  await page.reload()
  await page.frameLocator('iframe').getByText('Legacy preview fixture', { exact: true }).waitFor()
  assert.ok(
    (await page.locator('[data-task-workspace]').count()) > 0,
    'legacy preview restoration retains task panes',
  )
  await page.close()
  holdProjectA = true
  savedLayout = null
  const delayedPage = await browser.newPage({ viewport: { width: 1600, height: 1000 } })
  await delayedPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  delayedPage.on('pageerror', (error) => errors.push(error.message))
  delayedPage.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text())
  })
  await delayedPage.goto(`http://127.0.0.1:${address.port}/__workbench`)
  await delayedPage.locator('[data-task-workspace="a"]').waitFor()
  await Promise.all([
    delayedPage.waitForRequest((request) => request.url().endsWith('/a/vcs/changes')),
    delayedPage.getByRole('button', { name: '置顶摘要', exact: true }).click(),
  ])
  await delayedPage.evaluate(() => window.workbenchFixture.open('b'))
  await delayedPage.locator('[data-task-workspace="b"]').waitFor()
  await delayedPage.getByRole('button', { name: '置顶摘要', exact: true }).click()
  await delayedPage
    .getByRole('region', { name: '置顶摘要', exact: true })
    .getByText('Task B', { exact: true })
    .waitFor()
  releaseProjectA()
  holdProjectA = false
  await delayedPage
    .getByRole('region', { name: '置顶摘要', exact: true })
    .getByRole('button', { name: '项目变更', exact: false })
    .click()
  await delayedPage.getByRole('button', { name: 'b-only.ts M', exact: true }).waitFor()
  assert.equal(
    await delayedPage.getByRole('button', { name: 'src/one.ts M', exact: true }).count(),
    0,
    'late A response cannot overwrite B resources',
  )
  await delayedPage.getByRole('button', { name: '新建标签页', exact: true }).click()
  await delayedPage.getByRole('heading', { name: '新建标签页', exact: true }).waitFor()
  assert.equal(
    await delayedPage.getByRole('button', { name: '项目文件', exact: true }).count(),
    0,
    'new tabs must not expose the retired workspace browser',
  )
  await delayedPage.close()
  invalidLayout = true
  const invalidStart = requests.length
  const invalidPage = await browser.newPage({ viewport: { width: 1600, height: 1000 } })
  await invalidPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  invalidPage.on('pageerror', (error) => errors.push(error.message))
  invalidPage.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text())
  })
  await invalidPage.goto(`http://127.0.0.1:${address.port}/__workbench`)
  await invalidPage.locator('[data-task-workspace="a"]').waitFor()
  await invalidPage.evaluate(() => window.workbenchFixture.open('b', 'right'))
  await invalidPage.locator('[data-task-workspace="b"]').waitFor()
  await invalidPage.waitForTimeout(300)
  assert.equal(
    requests
      .slice(invalidStart)
      .some(
        (request) => request.method === 'PUT' && request.path === '/api/settings/chat-dock-layout',
      ),
    false,
  )
  await invalidPage.close()
  invalidLayout = false
  runningTask = true
  savedLayout = null
  const runningStart = requests.length
  const runningPage = await browser.newPage({ viewport: { width: 1600, height: 1000 } })
  await runningPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  runningPage.on('pageerror', (error) => errors.push(error.message))
  runningPage.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text())
  })
  await runningPage.goto(`http://127.0.0.1:${address.port}/__workbench`)
  await runningPage.locator('[data-task-workspace="a"]').waitFor()
  await runningPage.getByText('Running task output', { exact: true }).waitFor()
  await runningPage
    .locator('[data-task-workspace="a"]')
    .getByRole('button', { name: '项目与任务设置', exact: true })
    .click()
  await runningPage.getByRole('menuitem', { name: '关闭分屏', exact: true }).click()
  await runningPage.locator('[data-task-workspace="a"]').waitFor({ state: 'detached' })
  await runningPage.waitForRequest((request) => request.url().endsWith('/api/sessions/a/live'), {
    timeout: 6000,
  })
  assert.equal(
    requests.slice(runningStart).some((request) => request.path.endsWith('/abort')),
    false,
  )
  await runningPage.close()
  const reviewPage = await browser.newPage({ viewport: { width: 390, height: 844 } })
  await reviewPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  reviewPage.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text())
  })
  reviewPage.on('pageerror', (error) => errors.push(error.message))
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=focus`)
  await reviewPage.getByRole('button', { name: 'Open row', exact: true }).press('Enter')
  reviewExpect(
    await reviewPage
      .locator('[data-slot="page-detail"]')
      .evaluate((el) => el.contains(document.activeElement)),
    'R6: entering details transfers keyboard focus',
  )
  await reviewPage.getByRole('button', { name: 'Back to list', exact: true }).press('Enter')
  reviewExpect(
    await reviewPage.evaluate(() => document.activeElement?.id === 'fixture-row'),
    'R6: back restores list trigger focus',
  )
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=skills`)
  await reviewPage.getByText('npm:only-package', { exact: true }).waitFor({ state: 'attached' })
  const packageEntry = reviewPage.getByRole('button', { name: '已配置技能包', exact: true })
  const reachable = (await packageEntry.count()) > 0 && (await packageEntry.isVisible())
  reviewExpect(reachable, 'R7: package-only search remains reachable on mobile')
  if (reachable) {
    await packageEntry.click()
    await reviewPage.getByText('npm:only-package', { exact: true }).waitFor()
  }
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=snapshot`)
  await reviewPage.locator('button[title="snapshot.txt"]').click()
  await reviewPage.getByText('old snapshot', { exact: true }).waitFor()
  await Promise.all([
    reviewPage.waitForResponse((r) => r.url().endsWith('/snapshot/file-changes')),
    reviewPage.getByRole('button', { name: 'Start fixture run', exact: true }).click(),
  ])
  snapshotVersion = 1
  await Promise.all([
    reviewPage.waitForResponse((r) => r.url().endsWith('/snapshot/file-changes')),
    reviewPage.getByRole('button', { name: 'Finish fixture run', exact: true }).click(),
  ])
  await reviewPage.waitForTimeout(300)
  reviewExpect(
    (await reviewPage.getByText('new snapshot', { exact: true }).count()) > 0,
    'R2: completed run refreshes expanded snapshot diff',
  )
  await reviewPage.setViewportSize({ width: 1280, height: 900 })
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=navigation`)
  const sidebarToggle = reviewPage.locator('#workbench-sidebar-toggle')
  await sidebarToggle.waitFor()
  assert.equal(await sidebarToggle.count(), 1, 'Only one sidebar toggle is reachable')
  assert.equal(await sidebarToggle.getAttribute('aria-expanded'), 'true')
  assert.equal(
    await reviewPage.getByText('Fixture description', { exact: true }).isVisible(),
    false,
  )
  await sidebarToggle.focus()
  await sidebarToggle.press('Enter')
  await reviewPage.getByRole('button', { name: 'Show sidebar', exact: true }).waitFor()
  assert.equal(await sidebarToggle.getAttribute('aria-expanded'), 'false')
  assert.equal(
    await reviewPage
      .getByTestId('workbench-sidebar')
      .evaluate((sidebar) => Boolean(sidebar.closest('[inert]'))),
    true,
    'Collapsed navigation cannot receive keyboard focus',
  )
  reviewExpect(
    await sidebarToggle.evaluate((button) => button === document.activeElement),
    'Sidebar collapse preserves keyboard focus on the relocated toggle',
  )
  await sidebarToggle.press('Enter')
  await reviewPage.getByRole('button', { name: '收起侧栏', exact: true }).waitFor()
  reviewExpect(
    await sidebarToggle.evaluate((button) => button === document.activeElement),
    'Sidebar expansion restores keyboard focus to the sidebar heading toggle',
  )
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=welcome`)
  const previousIdeas = reviewPage.getByRole('button', { name: '上一组任务', exact: true })
  const nextIdeas = reviewPage.getByRole('button', { name: '下一组任务', exact: true })
  await previousIdeas.waitFor({ timeout: 5000 })
  assert.equal(await previousIdeas.isDisabled(), true)
  const ideas = reviewPage.getByRole('region', { name: '试试这些任务', exact: true })
  assert.equal(await ideas.getByRole('button', { name: '解释代码', exact: true }).count(), 1)
  await nextIdeas.press('Enter')
  await ideas.getByRole('button', { name: '制定计划', exact: true }).press('Enter')
  assert.equal(
    await reviewPage.getByRole('textbox', { name: 'Welcome draft' }).inputValue(),
    '为以下目标制定清晰、可执行的计划：',
  )
  await nextIdeas.press('Enter')
  assert.equal(await nextIdeas.isDisabled(), true)
  await ideas.getByRole('button', { name: '查找问题', exact: true }).waitFor()
  await previousIdeas.press('Enter')
  assert.equal(await nextIdeas.isDisabled(), false)
  await reviewPage.setViewportSize({ width: 390, height: 844 })
  assert.equal(
    await reviewPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
    true,
  )
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=message`)
  await reviewPage.evaluate(() => {
    window.__copiedReply = ''
    window.__denyCopy = false
    Object.defineProperty(navigator, 'clipboard', {
      configurable: true,
      value: {
        writeText: (value) => {
          if (window.__denyCopy) return Promise.reject(new Error('fixture denied'))
          window.__copiedReply = value
          return Promise.resolve()
        },
      },
    })
  })
  const copyReply = reviewPage.getByRole('button', { name: '复制回复', exact: true })
  await copyReply.waitFor({ timeout: 5000 })
  const copyBounds = await copyReply.boundingBox()
  assert.ok(copyBounds.width >= 44 && copyBounds.height >= 44)
  await copyReply.press('Enter')
  await reviewPage.getByRole('button', { name: '已复制', exact: true }).waitFor()
  assert.equal(await reviewPage.evaluate(() => window.__copiedReply), 'Reply **content**')
  const messageMore = reviewPage.getByRole('button', { name: '更多回复操作', exact: true })
  await messageMore.press('Enter')
  await reviewPage.getByRole('menuitem', { name: '从此处另开对话', exact: true }).click()
  assert.equal(await reviewPage.getByLabel('Fixture action').textContent(), 'child:boundary')
  await messageMore.press('Enter')
  await reviewPage.getByRole('menuitem', { name: '从此处继续', exact: true }).click()
  assert.equal(await reviewPage.getByLabel('Fixture action').textContent(), 'branch:boundary')
  await reviewPage.waitForFunction(
    () => document.activeElement?.getAttribute('aria-label') === '更多回复操作',
  )
  await reviewPage.getByRole('button', { name: '重试', exact: true }).click()
  assert.equal(await reviewPage.getByLabel('Fixture action').textContent(), 'retry')
  await reviewPage.getByRole('button', { name: 'Toggle fixture streaming' }).click()
  assert.equal(await messageMore.count(), 0)
  assert.equal(await copyReply.count(), 0)
  await reviewPage.getByRole('button', { name: 'Toggle fixture streaming' }).click()
  await reviewPage.evaluate(() => {
    window.__denyCopy = true
  })
  await copyReply.click()
  await reviewPage
    .getByRole('status')
    .filter({ hasText: '复制失败，请重试或手动选择文字。' })
    .waitFor()
  assert.equal(await reviewPage.getByRole('button', { name: '已复制', exact: true }).count(), 0)
  await reviewPage.evaluate(() => {
    window.__denyCopy = false
  })
  await copyReply.click()
  await reviewPage.getByRole('button', { name: '已复制', exact: true }).waitFor()
  const userMessage = reviewPage.locator('[data-pisper-message-id="user"]')
  const copyMessage = userMessage.getByRole('button', { name: '复制消息', exact: true })
  await copyMessage.waitFor({ timeout: 5000 })
  const userCopyBounds = await copyMessage.boundingBox()
  assert.ok(userCopyBounds.width >= 44 && userCopyBounds.height >= 44)
  await copyMessage.click()
  await userMessage.getByRole('button', { name: '已复制', exact: true }).waitFor()
  assert.equal(await reviewPage.evaluate(() => window.__copiedReply), 'Question **source**')
  await reviewPage.setViewportSize({ width: 1440, height: 900 })
  await reviewPage.mouse.move(1, 1)
  await reviewPage.getByRole('button', { name: 'Toggle fixture streaming' }).focus()
  const userActions = userMessage.locator('.message-actions')
  await reviewPage.waitForFunction(() => {
    const actions = document.querySelector('[data-pisper-message-id="user"] .message-actions')
    return actions && getComputedStyle(actions).opacity === '0'
  })
  await userMessage.hover()
  assert.equal(await userActions.evaluate((element) => getComputedStyle(element).opacity), '1')
  await reviewPage.mouse.move(1, 1)
  await reviewPage.getByRole('button', { name: 'Toggle fixture streaming' }).focus()
  await reviewPage.keyboard.press('Tab')
  assert.equal(
    await userMessage.getByRole('button').evaluate((element) => element === document.activeElement),
    true,
  )
  assert.equal(await userActions.evaluate((element) => getComputedStyle(element).opacity), '1')
  await reviewPage.setViewportSize({ width: 390, height: 844 })
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=message&typography=1`)
  const prose = reviewPage.locator('[data-pisper-message-id="reply"] .markdown-body')
  await prose.getByRole('heading', { name: '整理桌面，让工作更高效' }).waitFor()
  const proseMetrics = await prose.evaluate((element) => {
    const style = getComputedStyle(element)
    return { font: parseFloat(style.fontSize), line: parseFloat(style.lineHeight) }
  })
  assert.equal(proseMetrics.font, 16)
  assert.equal(proseMetrics.line, 26)
  const listSpacing = await prose.locator('li').evaluateAll((items) => {
    const first = items[0].getBoundingClientRect()
    const second = items[1].getBoundingClientRect()
    return { gap: second.top - first.bottom, height: first.height }
  })
  assert.ok(
    listSpacing.gap <= 1,
    `single-line list items should share the text rhythm: ${listSpacing.gap}`,
  )
  assert.ok(listSpacing.height <= proseMetrics.line + 1)
  for (const width of [320, 390, 768, 1440]) {
    await reviewPage.setViewportSize({ width, height: 844 })
    await reviewPage.evaluate(() => {
      document.documentElement.dataset.fontScale = 'large'
    })
    assert.equal(
      await reviewPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
      true,
    )
    assert.ok(
      (await prose.evaluate((element) => parseFloat(getComputedStyle(element).fontSize))) > 16,
    )
  }
  await reviewPage.screenshot({ path: join(output, 'message-typography.png') })
  await reviewPage.setViewportSize({ width: 390, height: 844 })
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=appearance`)
  const languagePicker = reviewPage.getByRole('combobox', { name: '界面语言', exact: true })
  await languagePicker.waitFor({ timeout: 5000 })
  await languagePicker.press('Enter')
  await reviewPage.getByRole('option', { name: 'English', exact: true }).waitFor()
  await reviewPage.keyboard.press('Escape')
  await reviewPage.waitForFunction(
    () => document.activeElement?.getAttribute('role') === 'combobox',
  )
  await languagePicker.press('Enter')
  await reviewPage.getByRole('option', { name: 'English', exact: true }).waitFor()
  await reviewPage.keyboard.down('End')
  await reviewPage.waitForFunction(() => document.activeElement?.textContent === 'English')
  await reviewPage.keyboard.up('End')
  await reviewPage.keyboard.press('Enter')
  const englishPicker = reviewPage.getByRole('combobox', { name: 'Display language', exact: true })
  await englishPicker.waitFor()
  await reviewPage.reload()
  await englishPicker.waitFor()
  for (const width of [320, 390]) {
    await reviewPage.setViewportSize({ width, height: 844 })
    const rect = await englishPicker.boundingBox()
    assert.ok(rect.height >= 44 && rect.x >= 0 && rect.x + rect.width <= width)
    const languageGap = await englishPicker.evaluate((element) => {
      const hint = document.getElementById(element.getAttribute('aria-describedby'))
      return element.getBoundingClientRect().top - hint.getBoundingClientRect().bottom
    })
    assert.ok(languageGap <= 13, 'language control must not have an empty spacer above it')
    await englishPicker.click()
    const options = reviewPage.getByRole('listbox')
    await options.waitFor()
    await options.evaluate(async (element) => {
      element.getBoundingClientRect()
      await Promise.all(
        element.getAnimations().map((animation) => animation.finished.catch(() => {})),
      )
    })
    const menu = await options.boundingBox()
    assert.ok(menu.x >= 0 && menu.x + menu.width <= width)
    assert.ok((await reviewPage.getByRole('option').first().boundingBox()).height >= 44)
    await reviewPage.keyboard.press('Escape')
  }
  await englishPicker.press('Enter')
  await reviewPage.getByRole('option', { name: 'Simplified Chinese', exact: true }).waitFor()
  await reviewPage.keyboard.down('Home')
  await reviewPage.waitForFunction(
    () => document.activeElement?.textContent === 'Simplified Chinese',
  )
  await reviewPage.keyboard.up('Home')
  await reviewPage.keyboard.press('Enter')
  await languagePicker.waitFor()
  const advancedSettings = reviewPage.getByText('高级', { exact: true })
  await advancedSettings.waitFor({ timeout: 5000 })
  const fontGroup = reviewPage.getByRole('radiogroup', { name: '字体大小', exact: true })
  assert.equal(await fontGroup.isVisible(), false)
  await advancedSettings.press('Enter')
  await fontGroup.waitFor()
  for (const label of ['颜色模式', '字体大小', '圆角风格', '界面密度', '界面动效']) {
    const group = reviewPage.getByRole('radiogroup', { name: label, exact: true })
    await group.waitFor({ timeout: 5000 })
    const radios = group.getByRole('radio')
    const selected = group.getByRole('radio', { checked: true })
    await selected.focus()
    assert.equal(
      await radios.evaluateAll((items) => items.filter((item) => item.tabIndex === 0).length),
      1,
      `${label}: one keyboard entry`,
    )
    const current = await radios.evaluateAll((items) =>
      items.findIndex((item) => item.getAttribute('aria-checked') === 'true'),
    )
    const next = radios.nth((current + 1) % (await radios.count()))
    const nextHandle = await next.elementHandle()
    const selectedHandle = await selected.elementHandle()
    // Radix 在下一事件循环移动焦点；按下后等待选择，再抬键，模拟完整键盘动作。
    await reviewPage.keyboard.down('ArrowRight')
    await reviewPage.waitForFunction(
      (element) => element.getAttribute('aria-checked') === 'true',
      nextHandle,
    )
    await reviewPage.keyboard.up('ArrowRight')
    assert.equal(await nextHandle.evaluate((element) => element === document.activeElement), true)
    await reviewPage.keyboard.down('ArrowLeft')
    await reviewPage.waitForFunction(
      (element) => element.getAttribute('aria-checked') === 'true',
      selectedHandle,
    )
    await reviewPage.keyboard.up('ArrowLeft')
  }
  await fontGroup.getByRole('radio', { name: '大', exact: true }).click()
  await advancedSettings.press('Space')
  assert.equal(await fontGroup.isVisible(), false)
  await reviewPage.getByRole('button', { name: 'Locate font setting' }).click()
  await fontGroup.waitFor()
  assert.equal(await fontGroup.getByRole('radio', { name: '大', exact: true }).isChecked(), true)
  await fontGroup.getByRole('radio', { name: '标准', exact: true }).click()
  const accentPicker = reviewPage.getByRole('combobox', { name: '强调色', exact: true })
  await accentPicker.click()
  await reviewPage.getByRole('option', { name: '自定义', exact: true }).click()
  const hexInput = reviewPage.getByRole('textbox', { name: '输入 HEX 颜色', exact: true })
  await hexInput.fill('#broken')
  assert.equal(await hexInput.getAttribute('aria-invalid'), 'true')
  await hexInput.fill('#007acc')
  assert.equal(await hexInput.getAttribute('aria-invalid'), 'false')
  await reviewPage.setViewportSize({ width: 320, height: 844 })
  assert.equal(
    await reviewPage.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
    true,
  )
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=appearance-navigation`)
  await reviewPage.getByRole('button', { name: 'Locate font setting' }).click()
  await reviewPage.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await reviewPage.getByRole('button', { name: 'Finish navigation' }).click()
  await reviewPage
    .getByRole('radiogroup', { name: '字体大小', exact: true })
    .waitFor({ timeout: 5000 })
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=drawer`)
  const drawerToggle = reviewPage.getByRole('button', { name: 'Show sidebar', exact: true })
  await drawerToggle.click()
  const drawer = reviewPage.locator('[data-sidebar="sidebar"][data-mobile="true"]')
  await drawer.waitFor()
  await reviewPage.waitForFunction(() =>
    document.querySelector('[data-mobile="true"]')?.contains(document.activeElement),
  )
  await reviewPage.keyboard.press('Escape')
  await drawer.waitFor({ state: 'hidden' })
  await reviewPage.waitForFunction(() => document.activeElement?.id === 'workbench-sidebar-toggle')
  await reviewPage.setViewportSize({ width: 574, height: 844 })
  await drawerToggle.click()
  await drawer.waitFor()
  await drawer.evaluate(async (element) => {
    element.getBoundingClientRect()
    await Promise.all(
      element.getAnimations().map((animation) => animation.finished.catch(() => {})),
    )
  })
  const drawerRect = await drawer.boundingBox()
  assert.equal(Math.round(drawerRect.width), 280)
  await reviewPage.locator('[data-slot="sheet-overlay"]').click({ position: { x: 540, y: 400 } })
  await drawer.waitFor({ state: 'hidden' })
  await reviewPage.close()
  assert.deepEqual(
    requests.filter((request) =>
      ['/api/workspace-entries', '/api/directories'].includes(request.path),
    ),
    [],
    'workbench actions must not request the retired directory API',
  )
  assert.deepEqual(reviewFailures, [])
  assert.deepEqual(errors, [])
  console.log(
    'Workbench: task splits, drafts, last-scroll navigation race, tabs, resource summary, per-file diff, sources, delayed task responses, native terminal ownership, mobile round trip and snapshot fallback passed.',
  )
} catch (error) {
  console.error('Workbench API paths:', [...new Set(requests.map((request) => request.path))])
  throw error
} finally {
  await browser?.close()
  await server.close()
  console.log(`Workbench artifacts: ${output}`)
}
