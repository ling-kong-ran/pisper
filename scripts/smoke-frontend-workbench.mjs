import assert from 'node:assert/strict'
import { readdir, readFile, mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer } from 'vite'
import { chromium } from 'playwright-core'

// 新外壳工作台验收：看板（/board）→ 会话页（/board/:id，单会话视图，不挂载 Dockview）。
// 会话时间相对当前时间生成，四个任务都落在看板的“已完成”泳道里，可以从卡片打开。
const output = await mkdtemp(join(tmpdir(), 'pisper-workbench-'))
const fixtureNow = Date.now()
// 已完成泳道默认只显示“今天”：刚过本地零点时把完成时间夹到当天。
const fixtureDayStart = Math.min(fixtureNow, new Date(fixtureNow).setHours(0, 0, 0, 0) + 1000)
const sessions = ['a', 'b', 'c', 'd'].map((id, index) => {
  const finished = new Date(
    Math.max(fixtureNow - (index + 1) * 60_000, fixtureDayStart),
  ).toISOString()
  return {
    id,
    name: `Task ${id.toUpperCase()}`,
    firstMessage: `Task ${id.toUpperCase()}`,
    messageCount: 32,
    cwd: `/fixture/project-${id === 'd' ? 'a' : id}`,
    created: new Date(fixtureNow - (index + 1) * 3_600_000).toISOString(),
    modified: finished,
    lastCompletedAt: finished,
    model: 'fixture/model',
    streaming: false,
  }
})
const requests = []
let runningTask = false
let snapshotVersion = 0
let holdProjectA = false
let releaseProjectA = () => {}
const distAssets = await readdir('dist/assets')
const css = distAssets.find((name) => /^index-.*\.css$/.test(name))
const fonts = new Set(distAssets.filter((name) => name.endsWith('.woff2')))
assert.ok(css, 'build production CSS before running workbench verification')
const server = await createServer({
  // 独立的依赖预构建缓存（同 replica 冒烟）：不与 dev 服务共用 node_modules/.vite，免得并发预构建互相改写。
  cacheDir: join(output, 'vite-cache'),
  server: { host: '127.0.0.1', port: 0, hmr: false, watch: null },
  appType: 'custom',
  plugins: [
    {
      name: 'workbench-fixture',
      configureServer(server) {
        server.middlewares.use(async (req, res, next) => {
          const url = new URL(req.url, 'http://fixture')
          // 生产 CSS 里的字体按站点根路径 /assets/ 引用；夹具从 dist/assets 原样提供。
          if (url.pathname.startsWith('/assets/') && fonts.has(url.pathname.slice(8))) {
            res.setHeader('Content-Type', 'font/woff2')
            res.end(await readFile(join('dist/assets', url.pathname.slice(8))))
            return
          }
          if (url.pathname === '/__preview') {
            res.setHeader('Content-Type', 'text/html')
            res.end('<p>Legacy preview fixture</p>')
            return
          }
          if (url.pathname === '/__workbench' || url.pathname === '/__review') {
            // 夹具页声明一个空图标：完整 Chrome（PISPER_UI_BROWSER_PATH）会主动请求 /favicon.ico，404 记成控制台错误，
            // 让“无控制台错误”的断言随浏览器种类变化；headless shell 不请求。
            const html = await server.transformIndexHtml(
              '/__workbench',
              `<!doctype html><html lang="zh-CN"><head><meta name="viewport" content="width=device-width,initial-scale=1"/><link rel="icon" href="data:,"/><link rel="stylesheet" href="/dist/assets/${css}"/></head><body><div id="root"></div><script type="module" src="/scripts/fixtures/${url.pathname === '/__review' ? 'frontend-review' : 'frontend-workbench'}.tsx"></script></body></html>`,
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
                ...(runningTask && session.id === 'a' ? { lastCompletedAt: undefined } : {}),
              })),
            }
          else if (url.pathname === '/api/client-info') data = { client: 'web' }
          // 会话页不再使用 Dock 布局；读取只返回空，写入由收尾断言确认从未发生。
          else if (url.pathname === '/api/settings/chat-dock-layout') data = null
          else if (url.pathname.endsWith('/tree'))
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
                // 第 50 行很长：统一视图不换行，差异区要横向滚动（R5 用它核对横向阅读位置）。
                Array.from(
                  { length: 100 },
                  (_, i) =>
                    '-old ' + i + '\n+new ' + i + (i === 50 ? ' ' + 'x'.repeat(320) : '') + '\n',
                ).join(''),
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
  browser = await chromium.launch({
    headless: true,
    ...(process.env.PISPER_UI_BROWSER_PATH
      ? { executablePath: process.env.PISPER_UI_BROWSER_PATH }
      : process.platform === 'win32'
        ? { channel: 'msedge' }
        : {}),
  })
  const fixtureUrl = (session) =>
    `http://127.0.0.1:${address.port}/__workbench${session ? `?session=${session}` : ''}`
  const nextFrame = (target) =>
    target.evaluate(
      () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
    )
  const errors = []
  const watch = (target) => {
    target.on('pageerror', (error) => errors.push(error.message))
    target.on('console', (message) => {
      // 资源加载失败只有通用文案，附上地址便于定位。
      if (message.type() === 'error')
        errors.push(`${message.text()} ${message.location()?.url || ''}`.trim())
    })
  }

  // 看板是首页：卡片来自同一份会话目录；打开卡片进入 /board/:id，返回条回到看板并把焦点还给来源卡片。
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, locale: 'zh-CN' })
  await page.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  watch(page)
  await page.goto(fixtureUrl())
  await page.locator('[data-board-root]').waitFor({ timeout: 45000 })
  for (const id of ['a', 'b', 'c', 'd'])
    await page.locator(`[data-board-card="${id}"][data-lane="done"]`).waitFor()
  await page.locator('[data-board-card="a"]').click()
  await page.locator('[data-task-workspace="a"]').waitFor({ timeout: 45000 })
  assert.equal(await page.evaluate(() => window.workbenchFixture.path()), '/board/a')
  assert.equal(await page.locator('[data-board-root]').count(), 0)
  assert.equal(
    await page.locator('.dv-dockview').count(),
    0,
    'the session page embeds the single-session view without Dockview',
  )
  // 规格 v2：会话页没有泳道条，去全部代理的入口在应用侧栏（这个夹具只挂路由、不挂外壳），
  // 这里直接 push 到 /board：焦点仍回到来源卡片。
  await page.getByText('Task A', { exact: true }).first().waitFor()
  assert.equal(await page.locator('[data-lane-strip], [data-board-entry]').count(), 0)
  await page.evaluate(() => window.workbenchFixture.board())
  await page.locator('[data-board-root]').waitFor()
  await page.waitForFunction(() =>
    document.activeElement?.closest('[data-board-card]')?.getAttribute('data-board-card'),
  )
  assert.equal(
    await page.evaluate(() =>
      document.activeElement?.closest('[data-board-card]')?.getAttribute('data-board-card'),
    ),
    'a',
    'returning to the board restores focus to the source card',
  )
  await page.locator('[data-board-card="b"]').focus()
  await page.keyboard.press('Enter')
  await page.locator('[data-task-workspace="b"]').waitFor()
  assert.equal(await page.evaluate(() => window.workbenchFixture.path()), '/board/b')

  // 后台任务的资源自动展示不能切走当前会话，也不能抢走输入焦点。
  const backgroundComposer = page.locator('[data-task-workspace="b"] .focus-composer textarea')
  await backgroundComposer.fill('继续编辑 B')
  await page.evaluate(() => window.workbenchFixture.revealFiles('a'))
  await nextFrame(page)
  assert.equal(await backgroundComposer.evaluate((el) => el === document.activeElement), true)
  assert.equal(await page.evaluate(() => window.workbenchFixture.path()), '/board/b')
  assert.equal(
    await page.locator('[data-task-workspace="a"]').count(),
    0,
    'Background resource reveal preserves the routed task',
  )
  // 会话页内部的“打开会话”事件经路由切换，地址栏始终是选中依据。
  await page.evaluate(() => window.workbenchFixture.select('a'))
  await page.locator('[data-task-workspace="a"]').waitFor()
  assert.equal(await page.evaluate(() => window.workbenchFixture.path()), '/board/a')
  assert.equal(await page.locator('[data-task-workspace]').count(), 1)
  // 第 3 期：会话页右侧是检查器（固定标签 改动 / 终端 / 文件 / 预览，没有可关闭的上下文页标签），
  // 与 52px 会话顶栏同一行起头；Dockview 与旧的“新建标签页”都不再挂载。规格 v2 第 4.4 节：检查器默认关着
  //（后台会话的资源自动展示也不会让它在打开会话时弹出来），由顶栏的改动按钮按需滑入。
  const initialPane = page.locator('[data-task-workspace="a"]')
  const inspectorOf = (owner) => owner.locator('[data-session-inspector]')
  const inspectorTab = (owner, id) => owner.locator(`[data-inspector-tab-id="${id}"]`)
  assert.equal(await inspectorOf(initialPane).count(), 0, 'the inspector starts closed')
  await initialPane.locator('[data-session-header] [data-inspector-toggle]').click()
  await inspectorOf(initialPane).waitFor()
  assert.equal(await initialPane.getAttribute('data-session-workspace'), 'aside')
  assert.equal(await inspectorTab(initialPane, 'changes').getAttribute('aria-selected'), 'true')
  assert.deepEqual(
    await inspectorOf(initialPane)
      .locator('[data-inspector-tab-id]')
      .evaluateAll((tabs) => tabs.map((tab) => tab.getAttribute('data-inspector-tab-id'))),
    ['changes', 'terminal', 'files', 'preview'],
  )
  const headerRow = await initialPane.evaluate((pane) => ({
    session: pane.querySelector('[data-session-header]')?.getBoundingClientRect().top,
    inspector: pane.querySelector('[data-session-inspector] header')?.getBoundingClientRect().top,
  }))
  assert.ok(
    Math.abs(headerRow.session - headerRow.inspector) <= 1,
    `the session header and the inspector tabs share the top row: ${JSON.stringify(headerRow)}`,
  )
  assert.equal(await page.getByRole('button', { name: '新增辅助页面', exact: true }).count(), 0)
  await page.evaluate(() => window.workbenchFixture.open('b'))
  await page.locator('[data-task-workspace="b"]').waitFor()
  assert.equal(await backgroundComposer.inputValue(), '继续编辑 B')
  await page.evaluate(() => window.workbenchFixture.open('a'))
  await initialPane.waitFor()

  await page.setViewportSize({ width: 320, height: 1000 })
  const mobileMore = page.getByRole('button', { name: '项目与任务设置', exact: true })
  await mobileMore.waitFor()
  await mobileMore.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => {}))),
  )
  const mobileMoreBounds = await mobileMore.boundingBox()
  assert.ok(mobileMoreBounds.width >= 44 && mobileMoreBounds.height >= 44)
  await page.setViewportSize({ width: 1600, height: 1000 })
  // 标题 ▾ 菜单（沿用旧“⋯”的 id）集中会话操作（规格 v2 第 2.5 节）：重命名、侧聊……；主题与终端不在这里
  //（⌘K、⌘` 与设置 › 界面）。单会话页没有分屏，也不再有“打开会话上下文”。
  await initialPane.locator('[data-session-header]').waitFor()
  await initialPane.locator('[data-session-header] [data-session-menu]').click()
  await page.getByRole('menuitem', { name: '重命名会话', exact: true }).waitFor()
  await page.getByRole('menuitem', { name: '侧聊', exact: true }).waitFor()
  assert.equal(await page.getByRole('menuitem', { name: /^主题：/ }).count(), 0)
  assert.equal(await initialPane.getByTestId('theme-toggle').count(), 0)
  assert.equal(await page.getByRole('menuitem', { name: /拆分到|关闭分屏/ }).count(), 0)
  assert.equal(await page.getByRole('menuitem', { name: '打开会话上下文', exact: true }).count(), 0)
  await page.keyboard.press('Escape')
  await page.waitForFunction(() => document.activeElement?.id === 'task-a-more')
  // 会话树唯一的入口是顶栏标题旁的 ⑂：打开同一个会话树对话框，关闭后焦点回到 ⑂。
  await initialPane.locator('[data-session-branches]').click()
  const recallDialog = page.getByRole('dialog', { name: '会话树', exact: true })
  await recallDialog.waitFor()
  await page.waitForFunction(() =>
    document.querySelector('[role="dialog"]')?.contains(document.activeElement),
  )
  await recallDialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-session-branches'))
  await initialPane.locator('.focus-composer textarea').fill('A 的未发送草稿')
  await initialPane.locator('.transcript').waitFor()

  // 改动 · 工作区：版本控制改动列表 + 所选文件差异（统一视图）。阅读位置按文件存进会话页，
  // 切到别的标签再回来时恢复，包括切换前最后一次还没派发 scroll 事件的位置（R5）。
  await inspectorOf(initialPane).getByRole('radio', { name: '工作区', exact: true }).click()
  const changesPane = initialPane.getByRole('region', { name: '项目变更', exact: true })
  await changesPane.getByRole('button', { name: 'src/one.ts M', exact: true }).click()
  assert.equal(
    await changesPane
      .getByRole('button', { name: 'src/one.ts M', exact: true })
      .getAttribute('aria-current'),
    'true',
  )
  const diffScroll = changesPane.locator('[data-change-detail="src/one.ts"] [data-diff-scroll]')
  await diffScroll.locator('[data-pisper-diff-row-count]').waitFor()
  assert.equal(
    await diffScroll.locator('[data-pisper-diff-row-count]').getAttribute('data-diff-mode'),
    'unified',
  )
  // 统一视图里长行保持单行，整个差异区横向滚动；用它同时核对纵横两个方向的阅读位置。
  const detail = changesPane.locator('[data-change-detail="src/one.ts"]')
  await diffScroll.evaluate(
    (el, id) => {
      el.scrollTop = 450
      el.scrollLeft = 160
      document.getElementById(id).click()
    },
    await inspectorTab(initialPane, 'files').getAttribute('id'),
  )
  await initialPane.getByRole('region', { name: '来源', exact: true }).waitFor()
  await inspectorTab(initialPane, 'changes').click()
  await diffScroll.locator('[data-pisper-diff-row-count]').waitFor()
  await nextFrame(page)
  const lastResourceOffset = await diffScroll.evaluate((el) => ({
    top: el.scrollTop,
    left: el.scrollLeft,
  }))
  reviewExpect(
    Math.abs(lastResourceOffset.top - 450) < 3 && Math.abs(lastResourceOffset.left - 160) < 3,
    'R5: resource tab must flush its last scroll before native scroll dispatch',
  )
  // 并排：默认 456 宽的检查器放不下两栏，“并排”禁用并在组上说明原因；拖宽到 600 以上后可用，
  // 两栏各占一半、新旧两侧都在视野里（不再是 880px 的并排渲染器把新文件一侧挤出视野）。
  const splitOption = detail.getByRole('radio', { name: '并排', exact: true })
  assert.equal(await splitOption.isDisabled(), true)
  assert.match(
    (await detail.locator('[data-slot="segmented-control"]').getAttribute('title')) || '',
    /并排/,
  )
  const inspectorSplitter = initialPane.getByRole('separator', {
    name: '调整会话上下文宽度',
    exact: true,
  })
  for (let step = 0; step < 3; step += 1) await inspectorSplitter.press('Shift+ArrowLeft')
  await page.waitForFunction(
    () =>
      document.querySelector('[data-change-detail="src/one.ts"]')?.getBoundingClientRect().width >=
      600,
  )
  await splitOption.click()
  await diffScroll.locator('[data-diff-mode="split-compact"]').waitFor()
  const splitFit = await diffScroll.evaluate((el) => {
    const box = el.getBoundingClientRect()
    const next = el.querySelector('.git-diff-cell.next')?.getBoundingClientRect()
    return {
      overflow: el.scrollWidth - el.clientWidth,
      nextInside: Boolean(next && next.left >= box.left && next.right <= box.right + 1),
    }
  })
  reviewExpect(
    splitFit.overflow <= 1 && splitFit.nextInside,
    `side-by-side keeps both columns in view inside the inspector: ${JSON.stringify(splitFit)}`,
  )
  // 视图偏好（统一 / 并排）是全局的：恢复默认的统一视图与默认宽度。
  await detail.getByRole('radio', { name: '统一', exact: true }).click()
  await diffScroll.locator('[data-diff-mode="unified"]').waitFor()
  await inspectorSplitter.dblclick()

  // 文件标签：资料来源（预览、无法预览的图片、经桌面桥接添加附件、搜索空结果）。
  await inspectorTab(initialPane, 'files').click()
  const sourcesPane = initialPane.getByRole('region', { name: '来源', exact: true })
  await sourcesPane.getByRole('button', { name: 'design.md', exact: true }).click()
  await sourcesPane.getByRole('button', { name: '预览', exact: true }).click()
  await page.getByRole('dialog').getByText('Source belongs to asset-a', { exact: true }).waitFor()
  assert.equal(await page.evaluate(() => window.__pisperRequestBack()), true)
  await page.getByRole('dialog').waitFor({ state: 'hidden' })
  assert.equal(await inspectorTab(initialPane, 'files').getAttribute('aria-selected'), 'true')
  await sourcesPane.getByRole('button', { name: 'broken.png', exact: true }).click()
  await sourcesPane.getByRole('button', { name: '预览', exact: true }).click()
  await page.getByRole('dialog').getByText('此文件暂不支持内容预览。', { exact: true }).waitFor()
  await page.getByRole('dialog').getByRole('button', { name: '关闭预览', exact: true }).click()
  // 来源页复用真实系统附件动作；取消、错误与浏览器选择由移动壳验收覆盖。
  // 桥接只为这一步临时装上：会话页按 window.pisperDesktop 判断桌面壳（窄化的桌面窗口不进手机外壳），
  // 后面的手机宽度检查需要普通浏览器，所以用完立即还原。
  await page.evaluate(() => {
    window.previousDesktopBridge = window.pisperDesktop
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
  await page.evaluate(() => {
    if (window.previousDesktopBridge === undefined) delete window.pisperDesktop
    else window.pisperDesktop = window.previousDesktopBridge
  })
  await sourcesPane.getByRole('textbox', { name: '搜索来源', exact: true }).fill('missing')
  await sourcesPane.getByText('没有匹配的来源', { exact: true }).waitFor()
  await sourcesPane.getByRole('textbox', { name: '搜索来源', exact: true }).fill('')

  // 会话间切换走路由：同一时刻只挂载一个会话视图，草稿、预览地址草稿与阅读位置按会话保留。
  await initialPane.locator('.transcript').evaluate((el) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: -1, bubbles: true }))
    el.scrollTop = 360
    el.dispatchEvent(new Event('scroll', { bubbles: true }))
  })
  await page.evaluate(() => window.workbenchFixture.open('b'))
  await page.locator('[data-task-workspace="b"]').waitFor()
  await page.evaluate(() => window.workbenchFixture.open('c'))
  await page.locator('[data-task-workspace="c"]').waitFor()
  assert.equal(await page.locator('[data-task-workspace]').count(), 1)
  await page.evaluate(() => window.workbenchFixture.open('a'))
  await page.locator('[data-task-workspace="a"]').waitFor()
  assert.equal(await page.locator('[data-task-workspace]').count(), 1)
  const pane = page.locator('[data-task-workspace="a"]')
  await inspectorTab(pane, 'preview').click()
  await pane.locator('[data-inspector-preview]').waitFor()
  // 检查器宽度：分隔条可用方向键与指针调整，双击恢复默认 456（宽度是全局偏好，不按会话保存）。
  const splitter = pane.getByRole('separator', { name: '调整会话上下文宽度', exact: true })
  const beforeWidth = Number(await splitter.getAttribute('aria-valuenow'))
  assert.equal(beforeWidth, 456)
  await splitter.press('ArrowLeft')
  assert.ok(Number(await splitter.getAttribute('aria-valuenow')) > beforeWidth)
  await splitter.press('ArrowRight')
  assert.equal(Number(await splitter.getAttribute('aria-valuenow')), beforeWidth)
  const handle = await splitter.boundingBox()
  await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2)
  await page.mouse.down()
  await page.mouse.move(handle.x - 28, handle.y + handle.height / 2)
  await page.mouse.up()
  assert.ok(
    Number(await splitter.getAttribute('aria-valuenow')) > beforeWidth,
    'pointer resize widens the inspector',
  )
  await splitter.dblclick()
  assert.equal(Number(await splitter.getAttribute('aria-valuenow')), 456)
  const addressInput = pane.locator('[data-inspector-preview] form input')
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
  await page.locator('[data-task-workspace="a"] [data-inspector-preview] form input').waitFor()
  assert.equal(await addressInput.inputValue(), 'https://example.invalid/kept-as-draft')
  assert.equal(await pane.locator('.focus-composer textarea').inputValue(), 'A 的未发送草稿')
  await page.waitForFunction(
    (top) =>
      Math.abs(document.querySelector('[data-task-workspace="a"] .transcript').scrollTop - top) < 4,
    savedReadingPosition,
  )
  await page.setViewportSize({ width: 390, height: 844 })
  // 第 4 期手机会话页：56px 顶栏（⋯ 是 44px 触控目标），没有泳道条与并排检查器；桌面检查器打开过的
  // 预览不会让手机落在只显示内容的视图上，对话与未发送草稿照常可见。
  await page.waitForFunction(
    () =>
      document.querySelector('#task-a-more')?.getBoundingClientRect().height >= 44 &&
      document.querySelector('[data-session-header="phone"]'),
  )
  assert.equal(await page.locator('[data-lane-strip], [data-session-inspector]').count(), 0)
  const mobilePane = page.locator('[data-task-workspace="a"]')
  await mobilePane.locator('.focus-composer textarea').waitFor({ state: 'visible' })
  assert.equal(await mobilePane.locator('.focus-composer textarea').inputValue(), 'A 的未发送草稿')
  // ⋯ → “改动、终端、文件与预览”：检查器是底部表单（模态、困住焦点、在可视区内）；
  // 系统返回键先关表单，资源页与草稿都保留，回到对话。
  await mobilePane.getByRole('button', { name: '项目与任务设置', exact: true }).click()
  await page.getByRole('menuitem', { name: '改动、终端、文件与预览', exact: true }).click()
  const sheet = page.locator('[data-inspector-sheet]')
  await sheet.waitFor()
  await sheet.evaluate((element) =>
    Promise.all(element.getAnimations().map((animation) => animation.finished.catch(() => {}))),
  )
  await page.screenshot({
    path: join(output, 'inspector-sheet-mobile.png'),
    animations: 'disabled',
  })
  const sheetBox = await sheet.boundingBox()
  assert.ok(
    sheetBox.x >= 0 && sheetBox.y >= 0 && sheetBox.y + sheetBox.height <= 845,
    'inspector sheet must fit inside the usable viewport',
  )
  await sheet.locator('button').first().focus()
  for (let index = 0; index < 10; index++) {
    await page.keyboard.press('Tab')
    assert.equal(
      await sheet.evaluate((el) => el.contains(document.activeElement)),
      true,
      'mobile inspector sheet traps keyboard focus',
    )
  }
  assert.equal(
    await page.locator('body').evaluate((el) => getComputedStyle(el).pointerEvents),
    'none',
    'background cannot receive pointer input while the sheet is modal',
  )
  const pagesBeforeBack = await page.evaluate(() => window.workbenchFixture.state().a.pages.length)
  assert.equal(await page.evaluate(() => window.__pisperRequestBack()), true)
  await sheet.waitFor({ state: 'detached' })
  assert.equal(
    await page.evaluate(() => window.workbenchFixture.state().a.pages.length),
    pagesBeforeBack,
    'back preserves resource pages',
  )
  await mobilePane.locator('.focus-composer textarea').waitFor({ state: 'visible' })
  assert.equal(await mobilePane.locator('.focus-composer textarea').inputValue(), 'A 的未发送草稿')
  await page.evaluate(() => window.workbenchFixture.open('b'))
  await page.locator('[data-task-workspace="b"]').waitFor()
  await page.setViewportSize({ width: 1600, height: 1000 })
  await page.waitForFunction(
    () => document.querySelector('#task-b-more')?.getBoundingClientRect().height < 44,
  )
  await nextFrame(page)
  reviewExpect(
    (await page.locator('[data-task-workspace="b"]').count()) === 1 &&
      (await page.evaluate(() => window.workbenchFixture.path())) === '/board/b',
    'R1: mobile task selection must survive desktop restoration',
  )
  await page.evaluate(() => window.workbenchFixture.open('a'))
  await page.locator('[data-task-workspace="a"]').waitFor()
  assert.equal(await page.locator('[data-task-workspace]').count(), 1)
  assert.equal(
    await page.locator('[data-task-workspace="a"] .focus-composer textarea').inputValue(),
    'A 的未发送草稿',
  )

  for (const width of [650, 660, 900, 390, 1600, 650, 660, 900, 390, 1600]) {
    await page.setViewportSize({ width, height: 1000 })
    await page.waitForTimeout(100)
  }
  // 宽度来回切换后回到桌面：已打开的检查器仍并排展开；收起后焦点回到顶栏的改动按钮，草稿不丢、不中止运行。
  const restored = page.locator('[data-task-workspace="a"]')
  await inspectorOf(restored).waitFor()
  assert.equal(await restored.getAttribute('data-session-workspace'), 'aside')
  await inspectorOf(restored).getByRole('button', { name: '收起检查器', exact: true }).click()
  await inspectorOf(restored).waitFor({ state: 'detached' })
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-inspector-toggle'))
  assert.equal(
    await page.locator('[data-task-workspace="a"] .focus-composer textarea').inputValue(),
    'A 的未发送草稿',
  )
  assert.equal(
    requests.some((request) => request.path.endsWith('/abort')),
    false,
  )
  await restored.locator('[data-inspector-toggle]').click()
  await inspectorOf(restored).waitFor()

  // 终端标签：终端属于会话，切走标签只卸载视图；进程子视图的筛选与跟随状态按会话保存。
  const terminalOwner = page.locator('[data-task-workspace="a"]')
  await inspectorTab(terminalOwner, 'terminal').click()
  await inspectorOf(terminalOwner).getByRole('radio', { name: '终端', exact: true }).click()
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
  await inspectorTab(terminalOwner, 'files').click()
  await terminalOwner.locator('[data-terminal-id]').waitFor({ state: 'detached' })
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
  await inspectorTab(terminalOwner, 'terminal').click()
  await inspectorOf(terminalOwner).getByRole('radio', { name: '进程', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '展开或收起日志', exact: true }).click()
  await terminalOwner.getByRole('checkbox', { name: '自动跟随输出', exact: true }).uncheck()
  await terminalOwner
    .getByRole('combobox', { name: '筛选进程状态', exact: true })
    .selectOption('running')
  await terminalOwner.getByRole('button', { name: '打开终端', exact: true }).click()
  await terminalOwner.locator('[data-terminal-id]').waitFor()
  assert.equal(
    await inspectorOf(terminalOwner)
      .getByRole('radio', { name: '终端', exact: true })
      .getAttribute('aria-checked'),
    'true',
  )
  assert.equal(
    await terminalOwner.locator('[data-terminal-id]').getAttribute('data-terminal-id'),
    terminalId,
  )
  await inspectorOf(terminalOwner).getByRole('radio', { name: '进程', exact: true }).click()
  assert.equal(
    await terminalOwner.getByRole('combobox', { name: '筛选进程状态', exact: true }).inputValue(),
    'running',
  )
  assert.equal(
    await terminalOwner.getByRole('checkbox', { name: '自动跟随输出', exact: true }).isChecked(),
    false,
  )
  assert.match(
    await inspectorOf(terminalOwner).locator('pre').innerText(),
    /output while view is closed/,
  )
  await inspectorOf(terminalOwner).getByRole('radio', { name: '终端', exact: true }).click()
  await terminalOwner.getByRole('button', { name: '停止进程', exact: true }).click()
  assert.deepEqual(await page.evaluate(() => window.terminalFixture.closed), [terminalId])
  // 离开会话页（换到别的会话）只卸载视图，终端进程归会话所有，回来后仍是同一个。
  await page.evaluate(() => window.workbenchFixture.open('c'))
  const secondOwner = page.locator('[data-task-workspace="c"]')
  await secondOwner.waitFor()
  await inspectorTab(secondOwner, 'terminal').waitFor()
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
  await page.evaluate(() => window.workbenchFixture.open('a'))
  await secondOwner.waitFor({ state: 'detached' })
  assert.deepEqual(await page.evaluate(() => window.terminalFixture.closed), [terminalId])
  await page.evaluate(
    (id) => window.terminalFixture.emit(id, 'continued in task C\r\n'),
    secondTerminalId,
  )
  await page.evaluate(() => window.workbenchFixture.open('c'))
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

  // A 与 D 共用 project-a：项目改动数据共享，但所选文件按会话各自保存（R4）。
  for (const id of ['a', 'd']) {
    await page.evaluate((target) => window.workbenchFixture.open(target), id)
    const owner = page.locator(`[data-task-workspace="${id}"]`)
    await inspectorTab(owner, 'changes').click()
    await inspectorOf(owner).getByRole('radio', { name: '工作区', exact: true }).click()
    await owner
      .getByRole('region', { name: '项目变更', exact: true })
      .getByText('/fixture/project-a', { exact: true })
      .waitFor()
  }
  await page.evaluate(() => window.workbenchFixture.open('a'))
  const sharedA = page.locator('[data-task-workspace="a"]')
  await sharedA.getByRole('button', { name: 'src/two.ts A', exact: true }).click()
  await sharedA.locator('[data-change-detail="src/two.ts"]').waitFor()
  await nextFrame(page)
  await page.evaluate(() => window.workbenchFixture.open('d'))
  const sharedD = page.locator('[data-task-workspace="d"]')
  await sharedD.locator('[data-change-detail]').waitFor()
  reviewExpect(
    (await sharedD
      .getByRole('button', { name: 'src/one.ts M', exact: true })
      .getAttribute('aria-current')) === 'true',
    'R4: selecting a file in A must not select it in D',
  )
  assert.equal(
    await sharedD
      .getByRole('button', { name: 'src/two.ts A', exact: true })
      .getAttribute('aria-current'),
    null,
    'shared project data does not share per-task file selection',
  )
  // 外链预览在会话页落到当前会话检查器的“预览”标签（没有 Dock 预览面板）。
  await page.evaluate(() => window.workbenchFixture.preview(location.origin + '/__preview'))
  await sharedD.locator('[data-inspector-preview]').waitFor()
  await sharedD
    .frameLocator('iframe')
    .getByText('Legacy preview fixture', { exact: true })
    .waitFor()
  assert.equal(await page.evaluate(() => window.workbenchFixture.path()), '/board/d')
  await page.close()

  holdProjectA = true
  const delayedPage = await browser.newPage({ viewport: { width: 1600, height: 1000 } })
  await delayedPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  watch(delayedPage)
  const heldProjectA = delayedPage.waitForRequest((request) =>
    request.url().endsWith('/a/vcs/changes'),
  )
  await delayedPage.goto(fixtureUrl('a'))
  await delayedPage.locator('[data-task-workspace="a"]').waitFor({ timeout: 45000 })
  // 新页面里检查器从关闭开始（规格 v2 第 4.4 节），先用顶栏的改动按钮打开。
  await delayedPage.locator('[data-task-workspace="a"] [data-inspector-toggle]').click()
  await delayedPage
    .locator('[data-session-inspector]')
    .getByRole('radio', { name: '工作区', exact: true })
    .click()
  await heldProjectA
  await delayedPage.evaluate(() => window.workbenchFixture.open('b'))
  await delayedPage.locator('[data-task-workspace="b"]').waitFor()
  await delayedPage.getByRole('button', { name: 'b-only.ts M', exact: true }).waitFor()
  releaseProjectA()
  holdProjectA = false
  await nextFrame(delayedPage)
  await delayedPage.waitForTimeout(300)
  await delayedPage.getByRole('button', { name: 'b-only.ts M', exact: true }).waitFor()
  assert.equal(
    await delayedPage.getByRole('button', { name: 'src/one.ts M', exact: true }).count(),
    0,
    'late A response cannot overwrite B resources',
  )
  // 检查器的固定标签与“+”说明都不提供已退役的工作区文件浏览器。
  assert.equal(await delayedPage.getByRole('tab', { name: '项目文件', exact: true }).count(), 0)
  await delayedPage.getByRole('button', { name: '添加组件标签', exact: true }).click()
  await delayedPage.locator('[data-slot="popover-content"]').waitFor()
  assert.equal(
    await delayedPage.getByRole('button', { name: '项目文件', exact: true }).count(),
    0,
    'the inspector must not expose the retired workspace browser',
  )
  await delayedPage.keyboard.press('Escape')
  await delayedPage.close()

  // 运行中的会话：离开会话页回到看板不会中止后端运行，卡片留在“运行中”泳道。
  runningTask = true
  const runningStart = requests.length
  const runningPage = await browser.newPage({ viewport: { width: 1600, height: 1000 } })
  await runningPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  watch(runningPage)
  await runningPage.goto(fixtureUrl('a'))
  await runningPage.locator('[data-task-workspace="a"]').waitFor({ timeout: 45000 })
  await runningPage.getByText('Running task output', { exact: true }).waitFor()
  await runningPage.evaluate(() => window.workbenchFixture.board())
  await runningPage.locator('[data-task-workspace="a"]').waitFor({ state: 'detached' })
  await runningPage.locator('[data-board-card="a"][data-lane="running"]').waitFor()
  await runningPage.waitForRequest(
    (request) => new URL(request.url()).pathname === '/api/sessions',
    { timeout: 6000 },
  )
  assert.equal(
    requests.slice(runningStart).some((request) => request.path.endsWith('/abort')),
    false,
  )
  await runningPage.close()
  runningTask = false
  assert.equal(
    requests.some(
      (request) => request.method === 'PUT' && request.path === '/api/settings/chat-dock-layout',
    ),
    false,
    'the session page never writes the retired Dock layout',
  )
  const reviewPage = await browser.newPage({ viewport: { width: 390, height: 844 } })
  await reviewPage.addInitScript(() =>
    window.addEventListener('error', (event) => console.error(event.message)),
  )
  reviewPage.on('console', (message) => {
    if (message.type() === 'error')
      errors.push(`${message.text()} ${message.location()?.url || ''}`.trim())
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
  // 规格 v2 的应用侧栏：任何时刻只有一个收起 / 展开键可达；按下后整块内容换成 60px 窄轨（分组区不再可聚焦），
  // 焦点留在换上来的展开键上；再展开回到收起键。
  await reviewPage.goto(`http://127.0.0.1:${address.port}/__review?mode=navigation`)
  const sidebarToggle = reviewPage.locator('[data-sidebar-toggle]')
  await sidebarToggle.waitFor()
  assert.equal(await sidebarToggle.count(), 1, 'Only one sidebar toggle is reachable')
  assert.equal(await sidebarToggle.getAttribute('aria-expanded'), 'true')
  assert.equal(
    await reviewPage.getByText('Fixture description', { exact: true }).isVisible(),
    false,
  )
  await sidebarToggle.focus()
  await sidebarToggle.press('Enter')
  await reviewPage.getByRole('button', { name: '展开侧栏', exact: true }).waitFor()
  assert.equal(
    await sidebarToggle.count(),
    1,
    'Only one sidebar toggle is reachable when collapsed',
  )
  assert.equal(await sidebarToggle.getAttribute('aria-expanded'), 'false')
  assert.equal(
    await reviewPage.getByTestId('fixture-groups').count(),
    0,
    'Collapsed navigation cannot receive keyboard focus',
  )
  // 宽度 180ms 过渡（规格 6.6）走完后是 60px 窄轨。
  await reviewPage.waitForFunction(
    () => document.querySelector('[data-shell-sidebar]')?.getBoundingClientRect().width === 60,
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
  // 规格 v2 第 4.2 节：“复制”取渲染后的纯文本，“⋯ → 复制为 Markdown”才复制原文。
  assert.equal(await reviewPage.evaluate(() => window.__copiedReply), 'Reply content')
  const messageMore = reviewPage.getByRole('button', { name: '更多回复操作', exact: true })
  await messageMore.press('Enter')
  await reviewPage.locator('[data-reply-copy-markdown]').click()
  await reviewPage.waitForFunction(() => window.__copiedReply === 'Reply **content**')
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
    Math.abs(listSpacing.gap - proseMetrics.font / 4) < 1,
    `adjacent list items follow the content-font quarter spacing: ${listSpacing.gap}`,
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
    assert.equal(
      await prose.evaluate((element) => parseFloat(getComputedStyle(element).fontSize)),
      16,
      'UI font size changes do not scale the separate content font role',
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
  // 界面字号已改为 11–16px 的数字输入（不再是“字体大小”单选组），其余外观选项仍是单选组。
  const fontInput = reviewPage.getByRole('spinbutton', { name: '界面字号', exact: true })
  assert.equal(await fontInput.isVisible(), false)
  await advancedSettings.press('Enter')
  await fontInput.waitFor()
  for (const label of ['颜色模式', '圆角风格', '界面密度', '界面动效']) {
    const group = reviewPage.getByRole('radiogroup', { name: label, exact: true })
    await group.waitFor({ timeout: 5000 })
    const radios = group.getByRole('radio')
    const selected = group.getByRole('radio', { checked: true })
    await selected.focus()
    const nativeGroup = await radios.evaluateAll((items) =>
      items.every((item) => item instanceof HTMLInputElement && item.type === 'radio'),
    )
    if (nativeGroup)
      assert.equal(
        await radios.evaluateAll((items) => new Set(items.map((item) => item.name)).size),
        1,
        `${label}: native radios share one keyboard group`,
      )
    else
      assert.equal(
        await radios.evaluateAll((items) => items.filter((item) => item.tabIndex === 0).length),
        1,
        `${label}: one keyboard entry`,
      )
    const current = await radios.evaluateAll((items) =>
      items.findIndex((item) =>
        item instanceof HTMLInputElement
          ? item.checked
          : item.getAttribute('aria-checked') === 'true',
      ),
    )
    const next = radios.nth((current + 1) % (await radios.count()))
    const nextHandle = await next.elementHandle()
    const selectedHandle = await selected.elementHandle()
    // Radix 在下一事件循环移动焦点；按下后等待选择，再抬键，模拟完整键盘动作。
    await reviewPage.keyboard.down('ArrowRight')
    await reviewPage.waitForFunction(
      (element) =>
        element instanceof HTMLInputElement
          ? element.checked
          : element.getAttribute('aria-checked') === 'true',
      nextHandle,
    )
    await reviewPage.keyboard.up('ArrowRight')
    assert.equal(await nextHandle.evaluate((element) => element === document.activeElement), true)
    await reviewPage.keyboard.down('ArrowLeft')
    await reviewPage.waitForFunction(
      (element) =>
        element instanceof HTMLInputElement
          ? element.checked
          : element.getAttribute('aria-checked') === 'true',
      selectedHandle,
    )
    await reviewPage.keyboard.up('ArrowLeft')
  }
  await fontInput.fill('16')
  await fontInput.press('Enter')
  await advancedSettings.press('Space')
  assert.equal(await fontInput.isVisible(), false)
  await reviewPage.getByRole('button', { name: 'Locate font setting' }).click()
  await fontInput.waitFor()
  assert.equal(await fontInput.inputValue(), '16')
  await fontInput.fill('14')
  await fontInput.press('Enter')
  const accentPicker = reviewPage.getByRole('combobox', { name: '强调色', exact: true })
  await accentPicker.click()
  await reviewPage.getByRole('option', { name: '自定义', exact: true }).click()
  await reviewPage.getByRole('button', { name: '自定义强调色', exact: true }).click()
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
    .getByRole('spinbutton', { name: '界面字号', exact: true })
    .waitFor({ timeout: 5000 })
  // 旧侧栏的手机抽屉（ui/sidebar 的 Sheet 形态）随 AppSidebar 退役；规格 v2 的手机抽屉在 A4 重做，届时补回验收。
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
    'Workbench: board to session routing, focus return, per-session drafts and last-scroll navigation race, session header menu and branch dialog focus, inspector tabs (changes with per-file diff and reading positions, terminal and processes, sources, preview), inspector width, delayed task responses, native terminal ownership, phone round trip and snapshot fallback passed.',
  )
} catch (error) {
  console.error('Workbench API paths:', [...new Set(requests.map((request) => request.path))])
  throw error
} finally {
  await browser?.close()
  await server.close()
  console.log(`Workbench artifacts: ${output}`)
}
