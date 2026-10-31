// 手机外壳冒烟（第 4 期重写）。两部分，都只用本机浏览器、不启动模型、不读用户配置：
// 1. 组件夹具（scripts/fixtures/mobile-shell.jsx）：新外壳仍在使用的真实组件——看板“新建项目…”、
//    工作目录选择器（含 Android SAF 导入）、附件的系统选择器、检查器底部表单里的改动与终端。
// 2. 真实生产构建（dist）+ 进程内隔离 Runtime（临时数据目录、随机端口）；全部代理、/live 与审批在浏览器
//    边界用 scripts/fixtures/board-fixture.mjs 固定，会话页的转录来自写进临时目录的真实会话：
//    - 390×844 手机 App（规格 v2 第 7 节）：主页（会话）草稿与返回根、顶栏 ☰（琥珀计数）+ 抽屉（入口、会话分组、
//      抽屉里一步处理普通批准）、安全区、软键盘、全部代理卡片（standard 一次允许只发 1 次 POST；critical 只有“打开处理”）、
//      派发底部表单、功能一列行与角标、组件列表 → 详情、设置列表 → 分区；
//    - 手机会话页：顶栏 ☰ / 标题 ▾ / 改动芯片，没有代理条；会话菜单与输入框芯片的底部表单；改动芯片打开检查器底部表单；
//      Android 返回键先关表单 / 抽屉再回上一级；
//    - 平板 1024×768（手机 App）：桌面布局、全部代理分组列表、检查器覆盖层；
//    - 窄宽度桌面模拟（pisperDesktop + customTitlebar）：不进手机外壳，标题栏无遮挡。
//    - 冷启动快捷键（手机与 1440 桌面）：外壳第一次画出来就按一次 ⌘N / ⌘1，生效且只生效一次；
//      全部代理上“会话主页”键紧接 ⌘N，请求等主页提交后交给草稿（快捷键 v2 里会话主页默认未绑定，
//      冒烟把它改绑到 ⌘9）。
// 浏览器模拟不等于真机（安全区、软键盘、返回键）或 Tauri / WebView 验证。
import assert from 'node:assert/strict'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { build } from 'esbuild'
import { chromium } from 'playwright-core'
import {
  BOARD_SESSION_ID,
  boardFixture,
  desktopBridgeScript,
  installBoardFixture,
  writeBoardSessionFixture,
} from './fixtures/board-fixture.mjs'
import { fetchIdempotent } from './fixtures/route-fetch.mjs'
import { SMOKE_BROWSER } from './fixtures/smoke-browser.mjs'

const root = dirname(dirname(fileURLToPath(import.meta.url)))
const output = await mkdtemp(join(tmpdir(), 'pisper-mobile-shell-'))
const report = { status: 'running', checks: [], pageErrors: [] }

// —— 1. 组件夹具 ——
const mocks = {
  '@/components/MarkdownMessage': 'export default function MarkdownMessage() { return null }',
  '@/app/use-i18n': `export function useI18n() { return {t: key => key, language: 'en-US'} }`,
  '@/features/chat/chat-api': `
    const data = () => ({files:[{path:'sample.txt',status:'modified',pending:true,approved:false,reverted:false,canRevert:true,snapshot:true,changeCount:1,added:1,removed:0}],summary:{files:window.evidence.revision,pending:1,added:1,removed:0}})
    export const chatApi = {
      getSessionFileChanges:async () => { window.evidence.reads++; return data() },
      getSessionFileChangeDiff:async () => ({diff:'--- a/sample.txt\\n+++ b/sample.txt\\n@@ -0,0 +1 @@\\n+fixture\\n',diffTruncated:false,found:true}),
      approveSessionFileChanges:async () => { window.evidence.approvals++; return data() },
      revertSessionFileChanges:async () => { window.evidence.reverts++; return {...data(),reverted:1} },
      getVcsChanges:async () => ({isRepo:false,files:[],diff:''}),
      getFileDiff:async () => ({diff:''}),
    }`,
  '@/features/chat/session-change-summary-api': `export async function invalidateSessionChangeSummary() {}`,
  // 没有桌面桥接时，同机 Runtime 的系统目录对话框接口不存在（404）：新建项目退回手动填写。
  '@/lib/api': `export async function apiJson(url) {
    if (url === '/api/local/directory-picker') throw new Error('Not found')
    throw new Error('Unexpected fixture API '+url)
  }`,
}
const mockPaths = new Map(
  Object.keys(mocks).map((name) => [join(root, 'src', name.slice(2)), name]),
)
const bundle = await build({
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
          // 领域模块用同目录相对导入；别名与相对路径应命中同一隔离边界。
          const mock = mocks[args.path]
            ? args.path
            : args.path.startsWith('.')
              ? mockPaths.get(resolve(args.resolveDir, args.path)) ||
                mockPaths.get(resolve(args.resolveDir, args.path).replace(/\.tsx?$/, ''))
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
const js = bundle.outputFiles.find((file) => file.path.endsWith('.js')).text
const assets = await readdir(join(root, 'dist/assets'))
assert.ok(
  assets.some((file) => /^index-.*\.css$/.test(file)),
  'Build production assets (npm run build) before running the mobile shell smoke',
)
const css = (
  await Promise.all(
    assets
      .filter((file) => file.endsWith('.css'))
      .map((file) => readFile(join(root, 'dist/assets', file), 'utf8')),
  )
).join('\n')
const fixtureServer = createServer((request, response) => {
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

// 改动页的批量操作（全部批准 / 全部还原）在底部 ⋯ 菜单里（规格 v2 第 4.4 节）：打开菜单、
// 等菜单项的禁用状态，再用 Esc 只关菜单（底部表单留着）。
const openChangesMenu = async (page) => {
  if (await page.locator('[role="menu"]').count()) return
  await page.locator('[data-fixture-sheet] [data-changes-more]').click()
  await page.locator('[role="menu"]').waitFor()
}
const waitMenuItemDisabled = async (page, label, disabled) => {
  await openChangesMenu(page)
  await page.waitForFunction(
    ([label, disabled]) => {
      const item = [...document.querySelectorAll('[role="menuitem"]')].find((node) =>
        node.textContent?.includes(label),
      )
      return Boolean(item) && (item.getAttribute('aria-disabled') === 'true') === disabled
    },
    [label, disabled],
  )
  await page.keyboard.press('Escape')
  await page.locator('[role="menu"]').waitFor({ state: 'detached' })
}
const chooseChangesMenuItem = async (page, label) => {
  await openChangesMenu(page)
  await page.getByRole('menuitem', { name: label, exact: true }).click()
}

async function verifyComponentFixture(browser) {
  await new Promise((done) => fixtureServer.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${fixtureServer.address().port}`
  const errors = []

  // 看板“新建项目…”：系统目录对话框取消、选中（登记并选为派发目标）、桥接报错、没有系统对话框时手动填写。
  const projectPage = await browser.newPage({ viewport: { width: 1200, height: 900 } })
  projectPage.setDefaultTimeout(5000)
  projectPage.on('pageerror', (error) => errors.push(`project: ${error.message}`))
  await projectPage.goto(`${origin}/?project`)
  const newProject = projectPage.getByRole('button', { name: 'board:project.new', exact: true })
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
  await projectPage.waitForFunction(() => !document.querySelector('button')?.disabled)
  assert.equal(
    await projectPage.getByRole('dialog').count(),
    0,
    'Native cancellation must not open a manual directory dialog',
  )
  assert.deepEqual(await projectPage.evaluate(() => window.readProjectState().registered), [])
  assert.equal(await projectPage.evaluate(() => window.projectEvidence.chosen), 0)
  await projectPage.evaluate(() => {
    window.projectPickResult = '/workspaces/native-project/'
  })
  await newProject.click()
  await projectPage.waitForFunction(() => window.projectEvidence.chosen === 1)
  assert.deepEqual(await projectPage.evaluate(() => window.readProjectState()), {
    registered: ['/workspaces/native-project'],
    launchCwd: '/workspaces/native-project',
  })
  await projectPage.evaluate(() => {
    window.pisperDesktop.pickDirectory = () => Promise.reject(new Error('native_directory_failed'))
  })
  await newProject.click()
  await projectPage.waitForFunction(() =>
    window.projectEvidence.notices.some(
      (notice) => notice.message === 'native_directory_failed' && notice.tone === 'error',
    ),
  )
  await projectPage.evaluate(() => {
    delete window.pisperDesktop.pickDirectory
  })
  await newProject.click()
  const manualPath = projectPage.getByRole('textbox', {
    name: 'common:workspacePicker.workingDirectoryPath',
    exact: true,
  })
  await manualPath.waitFor()
  await manualPath.fill('/workspaces/manual-project')
  await manualPath.press('Enter')
  await projectPage.getByRole('dialog').waitFor({ state: 'hidden' })
  await projectPage.waitForFunction(() => window.projectEvidence.chosen === 2)
  assert.deepEqual(await projectPage.evaluate(() => window.readProjectState()), {
    registered: ['/workspaces/manual-project', '/workspaces/native-project'],
    launchCwd: '/workspaces/manual-project',
  })
  await projectPage.close()
  report.checks.push(
    'Board 新建项目…: native cancel opens nothing and registers nothing; a picked directory is registered (normalized) and becomes the launch target; a bridge failure is reported; without a system dialog the manual WorkspacePicker registers the typed path',
  )

  const page = await browser.newPage({ viewport: { width: 390, height: 844 }, hasTouch: true })
  page.setDefaultTimeout(10000)
  page.on('pageerror', (error) => errors.push(error.message))
  await page.goto(origin)
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
  report.checks.push(
    'WorkspacePicker: a failed save keeps the typed path; Android SAF import only in local mode, cancellation keeps the path, an import fills it',
  )

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
  report.checks.push(
    'Attachments: web and the phone app use the system file chooser; the desktop bridge adds paths, ignores cancellation and reports failures',
  )

  // 检查器底部表单的“终端”：表单关闭即卸载，终端本身不随表单关闭；重新打开接回同一终端。
  await page.evaluate(() => window.controls.setSheet('terminal'))
  await page.getByRole('button', { name: 'terminal:workspace.create', exact: true }).click()
  await page.waitForFunction(() => window.evidence.created === 1)
  const terminal = page.locator('[data-terminal-id]')
  await terminal.waitFor()
  const terminalId = await terminal.getAttribute('data-terminal-id')
  assert.equal(await page.evaluate(() => window.evidence.terminalOptions[0].cwd), '/workspaces/one')
  await page.evaluate(() => window.controls.setSheet(null))
  await terminal.waitFor({ state: 'detached' })
  await page.evaluate(() => window.controls.setSheet('changes'))
  await page.waitForFunction(() => window.evidence.reads > 0)
  await page.evaluate(() => window.controls.setSheet('terminal'))
  await terminal.waitFor()
  assert.equal(await terminal.getAttribute('data-terminal-id'), terminalId)
  assert.equal(
    await page.evaluate(() => [window.evidence.created, window.evidence.closed].join()),
    '1,0',
    'Closing the sheet or switching tabs must keep the running terminal',
  )

  // 检查器底部表单的“改动”：运行中禁用写入；确认期间开始运行时取消旧的还原；表单关闭后不再写入。
  const approveLabel = 'chat:focusSession.fileChangesApproveAll'
  const revertLabel = 'board:changes.revertAll'
  await page.evaluate(() => window.controls.setSheet('changes'))
  await waitMenuItemDisabled(page, approveLabel, false)
  await page.evaluate(() => window.controls.setStreaming(true))
  await waitMenuItemDisabled(page, approveLabel, true)
  const reads = await page.evaluate(() => window.evidence.reads)
  await page.evaluate(() => {
    window.evidence.revision++
    window.controls.setStreaming(false)
  })
  await page.waitForFunction((previous) => window.evidence.reads > previous, reads)
  await waitMenuItemDisabled(page, approveLabel, false)
  await chooseChangesMenuItem(page, revertLabel)
  await page.waitForFunction(() => window.confirmPending)
  const readsBeforeConfirmRun = await page.evaluate(() => window.evidence.reads)
  await page.evaluate(() => window.controls.setStreaming(true))
  await page.waitForFunction((previous) => window.evidence.reads > previous, readsBeforeConfirmRun)
  await page.evaluate(async () => {
    window.resolveConfirm(true)
    await new Promise(requestAnimationFrame)
  })
  await page.evaluate(() => window.controls.setStreaming(false))
  await waitMenuItemDisabled(page, approveLabel, false)
  assert.equal(
    await page.evaluate(() => window.evidence.reverts),
    0,
    'A run starting during confirmation must cancel the stale revert',
  )
  await page.evaluate(() => {
    window.confirmPending = false
  })
  await chooseChangesMenuItem(page, revertLabel)
  await page.waitForFunction(() => window.confirmPending)
  await page.evaluate(() => window.controls.setSheet(null))
  await page.locator('[data-fixture-sheet]').waitFor({ state: 'detached' })
  await page.evaluate(async () => {
    window.resolveConfirm(true)
    await new Promise(requestAnimationFrame)
  })
  await page.waitForTimeout(100)
  assert.equal(
    await page.evaluate(() => window.evidence.reverts),
    0,
    'No stale writes after the sheet closes',
  )
  await page.evaluate(() => window.controls.setSheet('changes'))
  await waitMenuItemDisabled(page, approveLabel, false)
  await chooseChangesMenuItem(page, approveLabel)
  await page.waitForFunction(() => window.evidence.approvals === 1)
  report.checks.push(
    'Inspector sheet: the terminal survives closing the sheet and switching tabs; changes disable writes while running, cancel a revert when a run starts during confirmation, never write after the sheet closes, and approve once when idle',
  )
  await page.evaluate(() => window.controls.setMounted(false))
  await page.waitForFunction(() => window.evidence.closed === 1)
  await page.close()
  assert.deepEqual(errors, [])
}

// —— 2. 真实生产构建 + 隔离 Runtime ——
const dataDir = join(output, 'agent')
const workspace = join(output, 'workspace')
const fixtureHome = join(output, 'home')
await mkdir(workspace)
await mkdir(fixtureHome)
// CLI 扫描限于空的隔离 home；偏好经 Runtime 同步，也只写进这个临时目录。
process.env.HOME = fixtureHome
process.env.USERPROFILE = fixtureHome
process.env.CODEX_HOME = join(fixtureHome, '.codex')
process.env.CLAUDE_CONFIG_DIR = join(fixtureHome, '.claude')
process.env.PI_SKIP_VERSION_CHECK = '1'
process.env.PI_TELEMETRY = '0'
process.env.PISPER_AGENT_DIR = dataDir
process.env.PISPER_WORKSPACE_DIR = workspace
await writeBoardSessionFixture(join(dataDir, 'sessions'), workspace)
// 一个手放的原生组件（列表 → 详情 → 手机宽预览），外加一个旧版 iframe 组件：
// 负责人决定之前的组件都删除，Runtime 启动迁移把旧组件移进回收站，列表里不再出现它。
const componentDir = join(dataDir, 'custom-ui', 'game-asset-demo')
await mkdir(componentDir, { recursive: true })
await writeFile(
  join(componentDir, 'manifest.json'),
  JSON.stringify({
    runtime: 'native',
    name: '游戏素材看板',
    version: '1.2.0',
    description: '原生组件 · 手放',
    // 总览卡片与浮窗（A7 S6）：手机全部代理末尾全宽的组件卡；浮窗在手机外壳不显示。
    placements: [
      { surface: 'feature-page' },
      { surface: 'overview-card', sizes: ['S'] },
      { surface: 'floating' },
    ],
  }),
)
await writeFile(
  join(componentDir, 'component.js'),
  'export default function Card({ ctx }) {\n  return ctx.html`<p data-fixture-card>fixture component</p>`\n}\n',
)
const legacyDir = join(dataDir, 'custom-ui', 'old-iframe')
await mkdir(legacyDir, { recursive: true })
await writeFile(
  join(legacyDir, 'manifest.json'),
  JSON.stringify({ name: '旧版组件', entry: 'index.html', permissions: ['notify'] }),
)
await writeFile(join(legacyDir, 'index.html'), '<!doctype html><body>legacy</body>')

const now = Date.now()
const iso = (offset) => new Date(now + offset).toISOString()
const runningTools = [
  {
    type: 'tool',
    id: 'l1',
    name: 'bash',
    args: { command: 'npm test -- --runInBand' },
    status: 'running',
    startedAt: iso(-9_000),
    output: 'PASS src/auth/session.test.ts',
  },
]
const fileChanges = {
  files: [
    {
      path: 'src/auth/login.ts',
      status: 'modified',
      added: 24,
      removed: 9,
      changeCount: 2,
      changedAt: iso(-20_000),
      snapshot: true,
      canRevert: true,
      approved: true,
      reverted: false,
      pending: false,
    },
    {
      path: 'src/auth/session.test.ts',
      status: 'modified',
      added: 7,
      removed: 2,
      changeCount: 1,
      changedAt: iso(-60_000),
      snapshot: true,
      canRevert: true,
      approved: true,
      reverted: false,
      pending: false,
    },
  ],
  summary: { files: 2, pending: 0, added: 31, removed: 11 },
}

let app
let browser
let failurePage

async function startRuntime() {
  const { createPisperRuntime } = await import('../runtime/app-runtime.mjs')
  app = await createPisperRuntime({
    root,
    frontendRoot: join(root, 'dist'),
    runtimeCwd: workspace,
    dataDir,
    production: true,
    port: 0,
    host: '127.0.0.1',
    remote: { enabled: false },
  })
  await app.initialized
}

async function newContext(
  options,
  { bridge, mobileApp = false, safeArea = false, keyboard = false, sessions } = {},
) {
  const context = await browser.newContext({ locale: 'zh-CN', ...options })
  await context.addInitScript((shortcuts) => {
    try {
      localStorage.setItem('pisper-language', 'zh-CN')
      localStorage.setItem('pisper-model-onboarding-v1-dismissed', '1')
      localStorage.setItem('pisper-shortcuts', shortcuts)
    } catch {}
  }, SMOKE_SHORTCUTS)
  if (bridge) await context.addInitScript(bridge)
  if (mobileApp) await context.addInitScript('window.__PISPER_MOBILE_APP__ = true')
  if (safeArea)
    // 浏览器没有系统栏：模拟原生壳注入的安全区，检查最终位置而不是 CSS 写法。
    await context.addInitScript(() => {
      const apply = () => {
        const style = document.createElement('style')
        style.textContent =
          ':root{--pisper-safe-area-top:47px !important;--pisper-safe-area-bottom:34px !important}'
        document.head.appendChild(style)
      }
      if (document.head) apply()
      else document.addEventListener('DOMContentLoaded', apply)
    })
  if (keyboard)
    // 软键盘模拟：可控的 visualViewport（MobileViewportStabilizer 只读它的 height / offsetTop）。
    await context.addInitScript(() => {
      const target = new EventTarget()
      const state = { inset: 0 }
      Object.defineProperties(target, {
        height: { get: () => window.innerHeight - state.inset },
        width: { get: () => window.innerWidth },
        offsetTop: { get: () => 0 },
        offsetLeft: { get: () => 0 },
        pageTop: { get: () => 0 },
        pageLeft: { get: () => 0 },
        scale: { get: () => 1 },
      })
      Object.defineProperty(window, 'visualViewport', { configurable: true, get: () => target })
      window.__setKeyboard = (inset) => {
        state.inset = inset
        target.dispatchEvent(new Event('resize'))
      }
    })
  const fixture = await installBoardFixture(context, { sessions })
  await context.route('**/api/app-update*', (route) =>
    route.fulfill({ json: { state: 'current', checkedAt: Date.now() } }),
  )
  // 更多页角标：只在浏览器边界模拟四个读接口的计数。
  await context.route('**/api/workflows', (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({
          json: { workflows: [], runs: [], limits: { maxConcurrent: 4, running: 1 } },
        })
      : route.fallback(),
  )
  await context.route('**/api/schedules', (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({
          json: {
            tasks: [
              { id: 'a', enabled: true },
              { id: 'b', enabled: true },
              { id: 'c', enabled: false },
            ],
            runs: [],
          },
        })
      : route.fallback(),
  )
  await context.route('**/api/channels', (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({
          json: {
            providers: [],
            connections: {
              feishu: { enabled: true, status: 'failed' },
              weixin: { enabled: true, status: 'connected' },
              qq: null,
              telegram: null,
            },
            scopes: [],
            models: [],
          },
        })
      : route.fallback(),
  )
  await context.route('**/api/mcp?refresh=0', (route) =>
    route.fulfill({
      json: {
        services: [],
        tools: [],
        calls: [],
        metrics: {
          totalServices: 3,
          onlineServices: 3,
          availableTools: 0,
          restrictedTools: 0,
          errorRate: 0,
        },
      },
    }),
  )
  // 真实会话的运行态与改动：在浏览器边界补上运行中的工具与 2 个改动文件。
  await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/live`), async (route) => {
    const response = await fetchIdempotent(route)
    const live = await response.json().catch(() => ({}))
    await route.fulfill({
      response,
      json: {
        ...live,
        streaming: true,
        startedAt: iso(-12_000),
        lastActivityAt: new Date().toISOString(),
        tools: runningTools,
        activityFeed: runningTools,
        currentActivity: runningTools[0],
        approvals: [],
      },
    })
  })
  await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/change-summary`), (route) =>
    route.fulfill({
      json: {
        status: 'known',
        changedFiles: 2,
        pendingFiles: 0,
        added: 31,
        removed: 11,
        unknownFiles: 0,
        capped: false,
      },
    }),
  )
  await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/file-changes$`), (route) =>
    route.fulfill({ json: fileChanges }),
  )
  await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/file-changes/diff`), (route) =>
    route.fulfill({
      json: {
        diff: '--- a/src/auth/login.ts\n+++ b/src/auth/login.ts\n@@ -1 +1 @@\n-old\n+new\n',
        diffTruncated: false,
        found: true,
      },
    }),
  )
  const aborts = []
  await context.route(new RegExp(`/api/sessions/${BOARD_SESSION_ID}/abort$`), async (route) => {
    aborts.push(route.request().method())
    await route.fulfill({ json: { ok: true } })
  })
  return { context, fixture, aborts }
}

async function openPage(context, label, hash) {
  const page = await context.newPage()
  failurePage = page
  page.setDefaultTimeout(15000)
  page.on('pageerror', (error) => report.pageErrors.push(`${label}: ${error.message}`))
  await page.goto(app.url + '/' + hash)
  return page
}

async function settle(page, ms = 300) {
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  )
  await page.waitForTimeout(ms)
}

const hashOf = (page) => new URL(page.url()).hash
const nativeBack = (page) => page.evaluate(() => window.__pisperRequestBack?.())
const MOD_KEY = process.platform === 'darwin' ? 'Meta' : 'Control'
// 快捷键 v2：会话主页默认未绑定；冒烟把它改绑到 ⌘9（完整的合法绑定表，不依赖迁移）。
const { DEFAULT_SHORTCUTS } = await import('../shared/shortcuts.mjs')
const SMOKE_SHORTCUTS = JSON.stringify({ ...DEFAULT_SHORTCUTS, openSession: 'Mod+Digit9' })

// 冷启动探针（addInitScript 注入）：选择器第一次出现后的下一帧记为首次绘制，同时在捕获阶段只读地记下
// 第一个按键到达页面的时间。报告写明按键离首次绘制多久：快捷键层曾经懒加载，外壳画出来后约 0.3 秒
// 没有监听（React 揭示 Suspense 的节流），这段时间按下的键会直接丢失。
function firstPaintProbe(selector) {
  window.__firstPaint = { painted: 0, key: 0 }
  addEventListener('keydown', () => (window.__firstPaint.key ||= performance.now()), true)
  const observer = new MutationObserver(() => {
    if (!document.querySelector(selector)) return
    observer.disconnect()
    requestAnimationFrame(() => (window.__firstPaint.painted = performance.now()))
  })
  observer.observe(document, { childList: true, subtree: true })
}
const waitForFirstPaint = (page) =>
  page.waitForFunction(() => window.__firstPaint?.painted > 0, undefined, {
    polling: 'raf',
    timeout: 60000,
  })
// 首次绘制后立刻按一次（不重试）；返回按键距首次绘制的毫秒数。
async function pressAfterFirstPaint(page, key) {
  await waitForFirstPaint(page)
  await page.keyboard.press(key)
  return page.evaluate(() => Math.round(window.__firstPaint.key - window.__firstPaint.painted))
}
// 点亮的一级入口：桌面侧栏的入口（全部代理 → board、功能 → more、设置 → settings）；手机见 drawerCurrent。
const activeTabs = (page) =>
  page.evaluate(() =>
    [
      ...document.querySelectorAll(
        '[data-shell-sidebar] [data-sidebar-entry][aria-current="page"]',
      ),
    ].map(
      (tab) =>
        ({ board: 'board', features: 'more', settings: 'settings' })[tab.dataset.sidebarEntry],
    ),
  )
const noHorizontalScroll = (page) =>
  page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)

// 偏好镜像在同一个 Runtime 里按设备共享：换一台“设备”前清掉上次停留的会话，从全新设备开始。
async function resetLastSession() {
  const response = await fetch(app.url + '/api/local/browser-preferences', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      updates: { 'pisper-last-session': null },
      revisions: { 'pisper-last-session': Date.now() * 1000 + 999 },
    }),
  })
  assert.ok(response.ok, `reset last session: ${response.status}`)
}

const phoneViewport = {
  viewport: { width: 390, height: 844 },
  isMobile: true,
  hasTouch: true,
  deviceScaleFactor: 2,
  colorScheme: 'light',
}

// 手机外壳的一级入口点亮在抽屉里（规格 7.1）：打开抽屉读 aria-current，再用 Esc 关上（焦点回到 ☰）。
async function drawerCurrent(page) {
  await page.locator('[data-phone-menu]').first().click()
  const drawer = page.locator('[data-phone-drawer]')
  await drawer.waitFor()
  const current = await drawer
    .locator('[data-drawer-entry][aria-current="page"]')
    .evaluateAll((nodes) =>
      nodes.map(
        (node) =>
          ({ board: 'board', features: 'more', settings: 'settings' })[node.dataset.drawerEntry],
      ),
    )
  await page.keyboard.press('Escape')
  await drawer.waitFor({ state: 'detached' })
  return current
}

async function openDrawerEntry(page, entry) {
  await page.locator('[data-phone-menu]').first().click()
  const drawer = page.locator('[data-phone-drawer]')
  await drawer.waitFor()
  await drawer.locator(`[data-drawer-entry="${entry}"]`).click()
  await drawer.waitFor({ state: 'detached' })
}

// 需要你卡片的审批详情来自 /live（错峰拉取）：普通批准的“允许”可点、高风险的“高风险”说明出现后再操作。
async function waitForApprovalCards(page) {
  await page
    .locator('[data-board-card="fx-standard"][data-overview-card]')
    .getByRole('button', { name: /^允许/ })
    .and(page.locator(':enabled'))
    .waitFor({ timeout: 20000 })
  await page
    .locator('[data-board-card="fx-critical"][data-overview-card] [data-severity="critical"]')
    .waitFor({ timeout: 20000 })
}

async function verifyPhoneShell(boardSessions) {
  await resetLastSession()
  const { context, fixture } = await newContext(phoneViewport, {
    mobileApp: true,
    safeArea: true,
    keyboard: true,
    sessions: boardSessions,
  })
  // 主页是会话：全新设备落在新任务草稿（手机会话外壳）；主页是返回的根。
  // 冷启动：草稿第一次画出来就按一次硬件键盘 ⌘N。快捷键层随外壳同步加载、在提交阶段挂监听，草稿也在
  // 提交阶段认领新任务请求，所以这一下不会丢，也不会因为草稿还没登记而改开派发底部表单。
  await context.addInitScript(firstPaintProbe, '[data-new-task-draft="phone"]')
  const page = await openPage(context, 'phone', '#/')
  const draftFocused = () =>
    page.evaluate(
      () =>
        document.activeElement?.hasAttribute('data-launch-input') === true &&
        document.activeElement.closest('[data-new-task-draft="phone"]') !== null,
    )
  await waitForFirstPaint(page)
  // 手机草稿不自动聚焦（落地不弹软键盘）；硬件键盘的 ⌘N 是明确操作：聚焦草稿，不打开派发底部表单。
  assert.equal(await draftFocused(), false, 'the phone draft is not autofocused')
  const coldDelay = await pressAfterFirstPaint(page, MOD_KEY + '+KeyN')
  await page.waitForFunction(
    () => document.activeElement?.closest('[data-new-task-draft="phone"]') !== null,
    undefined,
    { timeout: 5000 },
  )
  assert.equal(
    await draftFocused(),
    true,
    'one hardware ⌘N right after the first paint focuses the draft',
  )
  await page.waitForTimeout(400)
  assert.equal(await page.getByRole('dialog').count(), 0, 'hardware ⌘N opens no dispatch sheet')
  await page.evaluate(
    () => document.activeElement instanceof HTMLElement && document.activeElement.blur(),
  )
  assert.equal(await draftFocused(), false)
  assert.equal(await page.locator('[data-session-shell="phone"]').count(), 1)
  assert.equal(await page.locator('[data-testid="mobile-tab-bar"], [data-shell-tab]').count(), 0)
  const homeMenu = await page.locator('[data-phone-menu]').boundingBox()
  assert.ok(homeMenu && homeMenu.height >= 43.5, `home ☰ ${JSON.stringify(homeMenu)}`)
  assert.equal(await nativeBack(page), false, 'Android back on the home draft hands over')
  report.checks.push(
    `Phone home: a fresh device lands on the new-task draft in the phone session shell, no tab bar (draft not autofocused; a single hardware ⌘N pressed ${coldDelay}ms after the draft's first paint focuses it without opening the dispatch sheet); ☰ is ≥44px; Android back on the home hands over to the platform`,
  )

  // 抽屉（规格 7.1）：min(320, 85vw)，搜索 · ✎ · 全部代理 · 功能 · 会话分组 · 设置；普通批准行下方常驻 44px 拒绝 / 允许，
  // 高风险与失败行只能点开；返回键先关抽屉，焦点回到 ☰。
  await page.locator('[data-phone-menu]').click()
  const drawer = page.locator('[data-phone-drawer]')
  await drawer.waitFor()
  await drawer.locator('[data-sidebar-row="fx-standard"]').waitFor({ timeout: 30000 })
  await drawer
    .locator('[data-sidebar-item="fx-standard"] [data-sidebar-drawer-actions]')
    .waitFor({ timeout: 20000 })
  const drawerFacts = await drawer.evaluate((panel) => {
    const height = (node) => Math.round(node.getBoundingClientRect().height)
    return {
      width: Math.round(panel.getBoundingClientRect().width),
      entries: [...panel.querySelectorAll('[data-drawer-entry]')].map((node) => [
        node.dataset.drawerEntry,
        height(node),
      ]),
      search: height(panel.querySelector('[data-drawer-search]')),
      rows: [...panel.querySelectorAll('[data-sidebar-row]')].map((row) => [
        row.dataset.sidebarGroupRow,
        height(row),
      ]),
      actions: [...panel.querySelectorAll('[data-sidebar-drawer-actions]')].map((node) => [
        node.closest('[data-sidebar-item]').dataset.sidebarItem,
        [...node.querySelectorAll('button')].map((button) => [button.textContent, height(button)]),
      ]),
    }
  })
  assert.equal(drawerFacts.width, 320)
  assert.deepEqual(drawerFacts.entries, [
    ['board', 44],
    ['features', 44],
    ['settings', 44],
  ])
  assert.equal(drawerFacts.search, 44)
  assert.ok(
    drawerFacts.rows.every(([group, h]) => h === (group === 'attention' ? 56 : 44)),
    JSON.stringify(drawerFacts.rows),
  )
  assert.deepEqual(drawerFacts.actions, [
    [
      'fx-standard',
      [
        ['拒绝', 44],
        ['允许', 44],
      ],
    ],
  ])
  await page.screenshot({ path: join(output, 'phone-drawer.png') })
  assert.equal(await nativeBack(page), true, 'Android back closes the drawer first')
  await drawer.waitFor({ state: 'detached' })
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-phone-menu'))
  assert.equal(hashOf(page), '#/')
  report.checks.push(
    'Phone drawer: 320 wide with a 44px search, 新任务, 全部代理 / 功能 / 设置 entries (44px), session rows 56 (需要你) / 44; only the standard approval carries 44px 拒绝 / 允许 (critical and failed rows only open); Android back closes it first and focus returns to ☰',
  )

  // 全部代理：顶栏 56（☰ + 琥珀“● 3”、页名、搜索、✎），44px 分段，需要你是卡片、其余是行；没有标签栏与标题栏。
  await openDrawerEntry(page, 'board')
  await page.waitForURL((url) => url.hash === '#/board')
  await page.locator('[data-overview-card]').first().waitFor({ timeout: 30000 })
  await waitForApprovalCards(page)
  assert.equal(
    await page.evaluate(() => document.documentElement.hasAttribute('data-phone-shell')),
    true,
  )
  assert.equal(await page.locator('[data-app-titlebar], [data-shell-sidebar]').count(), 0)
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-phone-topbar] [data-phone-menu]')
        ?.getAttribute('aria-label') === '打开导航，3 个需要你',
  )
  const top = await page.evaluate(() => {
    const bar = document.querySelector('[data-phone-topbar]')
    const box = (node) => {
      const rect = node.getBoundingClientRect()
      return { width: Math.round(rect.width), height: Math.round(rect.height) }
    }
    const segmented = document.querySelector(
      '[data-board-root] [role="radiogroup"], [data-board-root] [role="tablist"]',
    )
    return {
      height: Math.round(bar.getBoundingClientRect().height),
      top: Math.round(bar.getBoundingClientRect().top),
      title: bar.querySelector('[data-phone-title]').textContent,
      badge: bar.querySelector('[data-phone-attention]')?.textContent,
      search: box(bar.querySelector('[data-phone-search]')),
      newTask: box(bar.querySelector('[data-phone-new-task]')),
      segmented: segmented ? Math.round(segmented.getBoundingClientRect().height) : 0,
      cards: [...document.querySelectorAll('[data-overview-card]')].map(
        (card) => card.dataset.boardCard,
      ),
      rows: [...document.querySelectorAll('[data-overview-row]:not([data-overview-card])')].map(
        (row) => row.dataset.lane,
      ),
      h1: [...document.querySelectorAll('h1')].map((node) => node.textContent),
    }
  })
  assert.equal(top.height, 56)
  assert.equal(top.top, 47, 'the top bar sits under the top safe area')
  assert.equal(top.title, '全部代理')
  assert.equal(top.badge, '3')
  assert.deepEqual(top.search, { width: 44, height: 44 })
  assert.deepEqual(top.newTask, { width: 44, height: 44 })
  assert.equal(top.segmented, 44)
  assert.deepEqual(top.cards.toSorted(), ['fx-critical', 'fx-failed', 'fx-standard'])
  assert.ok(top.rows.length > 0 && !top.rows.includes('attention'), JSON.stringify(top.rows))
  assert.deepEqual(top.h1, ['全部代理'], 'one h1 (visually hidden) per page')
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'phone-board.png') })
  report.checks.push(
    'Phone 全部代理: 56px top bar under the 47px safe area with ☰ + amber 3, the page name, 44px search and ✎; a 44px 按状态 / 按项目 segment; the three 需要你 items are cards and the rest stay rows; one (visually hidden) h1; no tab bar, title bar or sidebar; no horizontal scroll',
  )

  // 卡片（规格 7.3）：高风险只有“打开处理”，不能一步批准；普通批准 [拒绝][允许] 44px，一次点击只发 1 次 POST；
  // 失败 [日志][重试] 44px。
  const critical = page.locator('[data-board-card="fx-critical"][data-overview-card]')
  assert.deepEqual(await critical.locator('[data-card-actions] button').allInnerTexts(), [
    '打开处理',
  ])
  assert.equal(await critical.getByRole('button', { name: /^允许/ }).count(), 0)
  const failedButtons = await page
    .locator('[data-board-card="fx-failed"] [data-card-actions] button')
    .evaluateAll((buttons) =>
      buttons.map((button) => [
        button.innerText.trim(),
        Math.round(button.getBoundingClientRect().height),
      ]),
    )
  assert.deepEqual(failedButtons, [
    ['日志', 44],
    ['重试', 44],
  ])
  const standardAllow = page
    .locator('[data-board-card="fx-standard"][data-overview-card]')
    .getByRole('button', { name: /^允许/ })
  const allowBox = await standardAllow.boundingBox()
  assert.ok(allowBox && allowBox.height >= 44, `approve touch target ${JSON.stringify(allowBox)}`)
  await standardAllow.click()
  for (let i = 0; i < 30 && fixture.posts.length < 1; i += 1) await page.waitForTimeout(100)
  await page.waitForTimeout(500)
  assert.deepEqual(
    fixture.posts.map((post) => [post.path, post.body]),
    [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
  )
  assert.equal(hashOf(page), '#/board', 'approving inline stays on 全部代理')
  report.checks.push(
    'Phone cards: critical shows only 打开处理 (no one-step approval anywhere on the page); failed shows 44px 日志 / 重试; standard 允许 (≥44px) sends exactly one POST {approved:true} and stays on 全部代理',
  )

  // 新任务：顶栏 ✎ 打开派发底部表单；返回键先关表单。
  await page.locator('[data-phone-topbar] [data-phone-new-task]').click()
  const dispatch = page.getByRole('dialog', { name: '新任务', exact: true })
  await dispatch.waitFor()
  await dispatch.locator('[data-launch-console="sheet"]').waitFor()
  // 输入框占满表单宽度：390 宽下不能被项目芯片挤到一百多像素宽。
  const sheetInput = await dispatch.locator('[data-launch-input]').boundingBox()
  assert.ok(sheetInput && sheetInput.width >= 300, `dispatch textarea width ${sheetInput?.width}`)
  assert.ok(sheetInput.height >= 44, `dispatch textarea height ${sheetInput.height}`)
  // A6 评审返修：与桌面派发同一条芯片工具行 [项目] [盾牌] [排队] … [模型] [派发]，每个 ≥44，一行放下（不换行）；
  // 没有“在 [项目] 中”前缀与单独的 Plan 芯片；说明指向抽屉的“运行中”（手机没有侧栏）。
  const sheetToolbar = await dispatch.locator('[data-launch-toolbar] button').evaluateAll((nodes) =>
    nodes.map((node) => {
      const box = node.getBoundingClientRect()
      return {
        name: node.getAttribute('aria-label') || node.textContent.trim(),
        width: box.width,
        height: box.height,
        top: Math.round(box.top),
      }
    }),
  )
  // A9：有运行中的会话且 Runtime 声明 sessionQueue 时多一枚“什么时候开始”（立即开始 / 排在…之后），手机上只留图标。
  assert.equal(sheetToolbar.length, 5, JSON.stringify(sheetToolbar))
  assert.match(sheetToolbar[0].name, /^项目/)
  assert.match(sheetToolbar[1].name, /^审批模式/)
  assert.match(sheetToolbar[2].name, /^什么时候开始 · 立即开始/)
  assert.match(sheetToolbar[3].name, /^模型/)
  assert.equal(sheetToolbar[4].name, '派发')
  for (const item of sheetToolbar)
    assert.ok(
      item.height >= 44 && item.width >= 44,
      `dispatch toolbar target ${JSON.stringify(item)}`,
    )
  assert.equal(new Set(sheetToolbar.map((item) => item.top)).size, 1, 'one toolbar row')
  assert.equal(await dispatch.getByText('在', { exact: true }).count(), 0)
  await dispatch.getByText('派发后留在当前页面，新代理会出现在抽屉的「运行中」。').waitFor()
  assert.equal(await nativeBack(page), true)
  await dispatch.waitFor({ state: 'detached' })
  // 关闭后焦点回到打开它的顶栏 ✎（Radix 只会还给 DialogTrigger，这里没有）。
  await page.waitForFunction(
    () => document.activeElement?.matches('[data-phone-topbar] [data-phone-new-task]'),
    undefined,
    { timeout: 3000 },
  )
  assert.equal(hashOf(page), '#/board')
  // 软键盘弹起：外壳随可视视口收缩（没有底部条要让位）；收起后回来。
  await page.evaluate(() => window.__setKeyboard(336))
  await page.waitForFunction(
    () => document.querySelector('.app-shell')?.dataset.mobileKeyboard === 'open',
  )
  assert.equal(await page.locator('[data-phone-topbar]').isVisible(), true)
  await page.evaluate(() => window.__setKeyboard(0))
  await page.waitForFunction(() => !document.querySelector('.app-shell')?.dataset.mobileKeyboard)
  report.checks.push(
    'Phone ✎ opens the dispatch bottom sheet with a full-width (≥300px, ≥44px tall) input and one toolbar row [项目] [盾牌] [模型] [派发] of ≥44 targets (Android back closes it first and focus returns to ✎); the software keyboard shrinks the shell and nothing sits at the bottom edge',
  )

  // 全部代理上“会话主页”键（改绑的 ⌘9）紧接 ⌘N：路由在 transition 里提交，按 ⌘N 时渲染的仍是全部代理（不认领，
  // 旧逻辑会打开派发底部表单）。外壳按路由器的最新去向判断，请求等主页提交后交给草稿：聚焦草稿、不开表单，只执行一次。
  await page.keyboard.press(`${MOD_KEY}+Digit9`)
  await page.keyboard.press(`${MOD_KEY}+KeyN`)
  await page.waitForURL((url) => url.hash === '#/')
  await page.waitForFunction(
    () => document.activeElement?.closest('[data-new-task-draft="phone"]') !== null,
    undefined,
    { timeout: 5000 },
  )
  await page.waitForTimeout(400)
  assert.equal(await page.getByRole('dialog').count(), 0, '⌘N during ⌘9 opens no dispatch sheet')
  assert.equal(hashOf(page), '#/')
  await page.evaluate(
    () => document.activeElement instanceof HTMLElement && document.activeElement.blur(),
  )
  report.checks.push(
    'Phone race: the rebound session-home key then ⌘N at once from 全部代理 lands on the home draft and focuses it (the request waits for the home to commit instead of opening the dispatch sheet over the leaving page)',
  )

  // 功能：一列 56 行与角标、组件工作室；组件列表 → 详情（页头“‹ 功能”44px，页名在顶栏）；返回键沿层级回到主页。
  await openDrawerEntry(page, 'features')
  await page.waitForURL((url) => url.hash === '#/more')
  await page.locator('[data-more-tile]').first().waitFor()
  await page.locator('[data-more-badge]').nth(3).waitFor({ timeout: 20000 })
  assert.equal(await page.locator('[data-phone-topbar] [data-phone-title]').textContent(), '功能')
  assert.deepEqual(await drawerCurrent(page), ['more'])
  assert.equal(
    await page.locator('[data-phone-menu]').getAttribute('aria-label'),
    '打开导航，2 个需要你',
    'the count follows the approval on every page',
  )
  const badges = await page
    .locator('[data-more-badge]')
    .evaluateAll((nodes) =>
      nodes.map(
        (node) =>
          `${node.closest('[data-more-tile]')?.dataset.moreTile}:${node.dataset.moreBadge}:${node.textContent.trim()}`,
      ),
    )
  assert.deepEqual(badges.toSorted(), [
    'page:channels:attn:1 断开',
    'page:mcp:neutral:3 在线',
    'page:schedules:neutral:2 启用',
    'page:workflows:run:1 运行中',
  ])
  const componentsTile = page.locator(
    '[data-more-group="extensions"] [data-more-tile="page:components"]',
  )
  await componentsTile.waitFor()
  assert.equal(
    await componentsTile.evaluate((node) => getComputedStyle(node).borderTopStyle),
    'solid',
  )
  assert.equal(
    await page
      .locator('[data-more-tile="add:component"]')
      .evaluate((node) => getComputedStyle(node).borderTopStyle),
    'dashed',
  )
  const phoneRows = await page.locator('[data-more-tile]').evaluateAll((nodes) =>
    nodes.map((node) => {
      const box = node.getBoundingClientRect()
      return { left: Math.round(box.left), width: Math.round(box.width), height: box.height }
    }),
  )
  assert.ok(
    phoneRows.every((row) => row.height >= 44 && row.left === phoneRows[0].left),
    JSON.stringify(phoneRows),
  )
  assert.equal(new Set(phoneRows.map((row) => row.width)).size, 1, JSON.stringify(phoneRows))
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'phone-more.png') })
  await componentsTile.click()
  await page.waitForURL((url) => url.hash === '#/components')
  const list = page.locator('[data-components-phone="list"]')
  await list.waitFor()
  assert.deepEqual(await drawerCurrent(page), ['more'], 'components keep 功能 lit')
  const row = page.locator('[data-component-row="game-asset-demo"]')
  const rowBox = await row.boundingBox()
  assert.ok(rowBox && rowBox.height >= 44, `component row ${JSON.stringify(rowBox)}`)
  // 旧版组件已被启动迁移移进回收站：列表里只有原生行，文件原样在 custom-ui-trash 里。
  assert.equal(await page.locator('[data-component-row]').count(), 1)
  assert.equal(await page.locator('[data-component-kind="legacy"]').count(), 0)
  assert.equal(existsSync(legacyDir), false, 'legacy component moved out of custom-ui/')
  const trashed = (await readdir(join(dataDir, 'custom-ui-trash'))).filter((name) =>
    name.startsWith('old-iframe-'),
  )
  assert.equal(trashed.length, 1, 'legacy component kept in the trash')
  assert.match(
    await readFile(join(dataDir, 'custom-ui-trash', trashed[0], 'component', 'index.html'), 'utf8'),
    /legacy/,
  )
  await row.click()
  await page.locator('[data-components-phone="detail"]').waitFor()
  assert.equal(hashOf(page), '#/components/game-asset-demo')
  // 手机宽预览：原生组件在真实宿主里渲染，不出现横向滚动。
  await page.locator('[data-component-preview] [data-fixture-card]').waitFor({ timeout: 30000 })
  assert.equal(await noHorizontalScroll(page), true)
  // 详情依次是预览、属性、能力、放置（规格 7.4）；放置磁贴触控 ≥44px，浮窗磁贴注明手机上不显示。
  const detailOrder = await page.evaluate(() => {
    const detail = document.querySelector('[data-components-phone="detail"]')
    const marks = ['[data-component-preview]', '[data-props-form]', '[data-placement-tile]']
    return marks.map((selector) => {
      const node = detail?.querySelector(selector)
      return node ? Math.round(node.getBoundingClientRect().top + window.scrollY) : -1
    })
  })
  assert.ok(
    detailOrder.every((top, index) => top >= 0 && (index === 0 || top > detailOrder[index - 1])),
    JSON.stringify(detailOrder),
  )
  const featureTile = page.locator('[data-placement-tile="feature-page"] button[role="checkbox"]')
  const featureTileBox = await featureTile.evaluate((node) => {
    const rect = node.closest('label').getBoundingClientRect()
    return { height: Math.round(rect.height) }
  })
  assert.ok(featureTileBox.height >= 44, JSON.stringify(featureTileBox))
  assert.match(
    await page.locator('[data-placement-tile="floating"]').textContent(),
    /浮窗|手机上不显示/,
  )
  // S7：提示词底部表单（规格 7.4）：详情里的“用提示词生成或修改”≥44px，打开底部表单、输入框可见；Esc 收起，不发送。
  const promptOpen = page.locator('[data-component-prompt-open]')
  const promptOpenBox = await promptOpen.boundingBox()
  assert.ok(promptOpenBox && promptOpenBox.height >= 44, JSON.stringify(promptOpenBox))
  await promptOpen.click()
  const promptSheet = page.locator('[data-component-prompt-sheet]')
  await promptSheet.locator('[data-component-prompt-input]').waitFor()
  const sheetBox = await promptSheet.boundingBox()
  assert.ok(sheetBox && Math.round(sheetBox.y + sheetBox.height) >= 843, JSON.stringify(sheetBox))
  assert.equal(await promptSheet.locator('[data-component-generate]').isDisabled(), true)
  await page.keyboard.press('Escape')
  await promptSheet.waitFor({ state: 'detached' })
  const phoneHeader = await page.evaluate(() => {
    const back = document.querySelector('[data-back-to-more]')
    const header = back?.closest('header')
    return {
      back: back ? Math.round(back.getBoundingClientRect().height) : 0,
      backText: back?.textContent?.trim(),
      theme: [...(header?.querySelectorAll('[data-testid="theme-toggle"]') ?? [])].filter(
        (node) => node.getClientRects().length > 0,
      ).length,
    }
  })
  assert.deepEqual(phoneHeader, { back: 44, backText: '功能', theme: 0 })
  // “‹ 全部组件”退回列表那一条历史：浏览器后退不会又回到详情。
  await page.getByRole('button', { name: '全部组件', exact: true }).click()
  await list.waitFor()
  assert.equal(hashOf(page), '#/components')
  await row.click()
  await page.locator('[data-components-phone="detail"]').waitFor()
  assert.equal(await nativeBack(page), true)
  await list.waitFor()
  assert.equal(hashOf(page), '#/components')
  assert.equal(await nativeBack(page), true)
  await page.waitForURL((url) => url.hash === '#/more')
  assert.equal(await nativeBack(page), true)
  await page.waitForURL((url) => url.hash === '#/')
  await page.locator('[data-new-task-draft="phone"]').waitFor()
  assert.equal(await nativeBack(page), false, 'the session home is the root')
  report.checks.push(
    'Phone 功能: page name in the top bar, one-column ≥44px rows with 1 运行中 / 2 启用 / 1 断开 (amber) / 3 在线 badges, the 组件工作室 row under 扩展 (solid) and the dashed + 放置组件; components list → detail (≥44px rows, 44px “‹ 功能”, the native preview renders, the detail reads preview → props → placements with ≥44px placement tiles and a ≥44px “用提示词生成或修改” that opens the bottom prompt sheet, and the legacy component was moved to the trash) with 功能 lit in the drawer; Android back walks detail → list → 功能 → home, and the home hands over to the platform',
  )

  // 全部代理末尾的组件卡（规格 3.4、7.3，A7 S6）：全宽、内容区高 120（S）、排在列表之后；
  // 浮窗实例在手机外壳不显示（实例保留，负责人决定）。看完移出，后面的平板与桌面检查不受影响。
  const phonePlaced = []
  for (const payload of [
    { componentId: 'game-asset-demo', surface: 'overview-card', size: 'S' },
    { componentId: 'game-asset-demo', surface: 'floating' },
  ]) {
    const result = await page.evaluate(async (body) => {
      const response = await fetch('/api/custom-ui/instances', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      })
      return { status: response.status, body: await response.json().catch(() => null) }
    }, payload)
    assert.equal(result.status, 201, JSON.stringify(result))
    phonePlaced.push(result.body.instance.instanceId)
  }
  // 看板事件流在这里是固定数据（没有 component_changed），实例缓存不会自己作废：重新加载页面读新实例。
  // 上面的手机预览刚写过安全模式哨兵（初始实例提交 3 秒后才清）：等它清掉再重新加载，免得被“上次可能卡死”询问。
  await page.waitForTimeout(3500)
  await page.reload()
  await page.locator('[data-phone-menu]').first().waitFor({ timeout: 30000 })
  await openDrawerEntry(page, 'board')
  await page.waitForURL((url) => url.hash === '#/board')
  const phoneCard = page.locator('[data-overview-widgets="phone"] [data-overview-component]')
  await phoneCard.locator('[data-fixture-card]').waitFor({ timeout: 30000 })
  await phoneCard.scrollIntoViewIfNeeded()
  const phoneCardLayout = await page.evaluate(() => {
    const card = document.querySelector('[data-overview-widgets="phone"] [data-overview-component]')
    const box = card.getBoundingClientRect()
    const body = card.querySelector('[data-overview-component-body]').getBoundingClientRect()
    const sections = [...document.querySelectorAll('[data-overview-section]')]
    const menu = card.querySelector('[data-overview-component-menu]').getBoundingClientRect()
    return {
      width: Math.round(box.width),
      gutter: Math.round(box.left),
      body: `${Math.round(body.width)}x${Math.round(body.height)}`,
      afterList: sections.length > 0 && box.top >= sections.at(-1).getBoundingClientRect().bottom,
      floating: document.querySelectorAll('[data-floating-custom-ui]').length,
      menu: Math.round(Math.min(menu.width, menu.height)),
    }
  })
  assert.deepEqual(phoneCardLayout, {
    width: 358,
    gutter: 16,
    body: '358x120',
    afterList: true,
    floating: 0,
    menu: 44,
  })
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'phone-board-components.png') })
  // 安全模式哨兵在初始实例提交 3 秒后清掉：等它清掉再移出，后面的上下文不会被“上次可能卡死”询问。
  await page.waitForTimeout(3500)
  for (const instanceId of phonePlaced) {
    const status = await page.evaluate(
      async (id) => (await fetch(`/api/custom-ui/instances/${id}`, { method: 'DELETE' })).status,
      instanceId,
    )
    assert.equal(status, 200)
  }
  await page.reload()
  await page.locator('[data-phone-menu]').first().waitFor({ timeout: 30000 })
  assert.equal(await phoneCard.count(), 0)
  report.checks.push(
    'Phone 全部代理: an overview-card instance renders after the list, full width (358 in the 16px gutters) with a 120 content area and a 44px ⋯; the floating instance stays hidden in the phone shell',
  )

  // 设置（规格 7.4）：抽屉底部“设置”进分区列表（56 行），点“界面”进分区，顶上“‹ 设置”；返回键从分区回列表。
  await openDrawerEntry(page, 'settings')
  await page.waitForURL((url) => url.hash === '#/config')
  const sections = page.locator('[data-mobile-navigation="settings"] [data-settings-section]')
  await sections.first().waitFor({ timeout: 30000 })
  const sectionRows = await sections.evaluateAll((nodes) =>
    nodes.map((node) => [
      node.dataset.settingsSection,
      Math.round(node.getBoundingClientRect().height),
    ]),
  )
  assert.deepEqual(
    sectionRows.map(([section]) => section),
    ['models', 'interface', 'notifications', 'shortcuts', 'updates', 'about'],
  )
  assert.ok(
    sectionRows.every(([, h]) => h >= 56),
    JSON.stringify(sectionRows),
  )
  assert.equal(await page.locator('[data-phone-topbar] [data-phone-title]').textContent(), '设置')
  assert.equal(await page.locator('[data-settings-features-hint]').count(), 1)
  await page.screenshot({ path: join(output, 'phone-settings-list.png') })
  await page.locator('[data-settings-section="interface"]').click()
  await page.waitForURL((url) => url.hash === '#/config/interface')
  await page.locator('[data-components-moved]').waitFor({ timeout: 30000 })
  const back = await page.locator('[data-settings-back]').boundingBox()
  assert.ok(back && back.height >= 44, `settings back ${JSON.stringify(back)}`)
  assert.equal(await page.getByRole('tab', { name: /独立组件/ }).count(), 0)
  assert.deepEqual(await drawerCurrent(page), ['settings'])
  assert.equal(await noHorizontalScroll(page), true)
  // A6：分区页与功能页同一模板——页名（短名“界面”）在外壳顶栏，页面 h1 只给读屏；设置搜索紧跟“‹ 设置”，触控 ≥44。
  assert.equal(await page.locator('[data-phone-title]').innerText(), '界面')
  const settingsHeading = page.locator('[data-settings-page="interface"] h1')
  assert.equal(await settingsHeading.innerText(), '界面')
  assert.match(await settingsHeading.getAttribute('class'), /\bsr-only\b/)
  const settingsSearch = await page.locator('[data-mobile-settings-search] input').boundingBox()
  assert.ok(
    settingsSearch && settingsSearch.height >= 44 && settingsSearch.y > back.y,
    `settings search ${JSON.stringify(settingsSearch)}`,
  )
  await page.screenshot({ path: join(output, 'phone-settings.png') })
  assert.equal(await nativeBack(page), true)
  await page.waitForURL((url) => url.hash === '#/config')
  await page.locator('[data-settings-section="interface"]').click()
  await page.locator('[data-settings-back]').click()
  await page.waitForURL((url) => url.hash === '#/config')
  await page.locator('[data-settings-section="interface"]').click()
  await page.locator('[data-components-moved]').click()
  await page.waitForURL((url) => url.hash === '#/components')
  report.checks.push(
    'Phone settings: the drawer opens the section list (模型 · 界面 · 通知 · 快捷键 · 更新 · 关于, rows ≥56, the “in 功能” hint); a section shows a 44px “‹ 设置” with the ≥44px settings search under it, the short page name in the top bar (h1 sr-only), and 设置 lit; Android back and “‹ 设置” return to the list; the standalone-widgets tab is replaced by a link to /components',
  )
  await context.close()
}

async function verifyPhoneSession(boardSessions) {
  await resetLastSession()
  const { context, fixture, aborts } = await newContext(phoneViewport, {
    mobileApp: true,
    safeArea: true,
    sessions: boardSessions,
  })
  const page = await openPage(context, 'phone session', '#/board')
  await page.locator('[data-phone-topbar]').waitFor({ timeout: 60000 })
  await waitForApprovalCards(page)
  await page.locator(`[data-board-card="${BOARD_SESSION_ID}"]`).click()
  await page.waitForURL((url) => url.hash === `#/board/${BOARD_SESSION_ID}`)
  const header = page.locator('[data-session-header="phone"]')
  await header.waitFor({ timeout: 30000 })
  await page.locator('[data-session-changes]').waitFor({ timeout: 30000 })
  const facts = await page.evaluate(() => {
    const box = (node) => {
      const rect = node.getBoundingClientRect()
      return { width: Math.round(rect.width), height: Math.round(rect.height) }
    }
    const menu = document.querySelector('[data-session-header="phone"] [data-phone-menu]')
    return {
      shell: document.querySelector('[data-session-shell]')?.dataset.sessionShell,
      workspace: document.querySelector('[data-session-workspace]')?.dataset.sessionWorkspace,
      tabBar: document.querySelectorAll('[data-testid="mobile-tab-bar"], [data-shell-tab]').length,
      strips: document.querySelectorAll('[data-lane-strip], [data-agent-strip], [data-agent-chip]')
        .length,
      headerHeight: Math.round(
        document.querySelector('[data-session-header="phone"]').getBoundingClientRect().height,
      ),
      menu: { label: menu.getAttribute('aria-label'), ...box(menu) },
      title: box(document.querySelector('[data-session-menu-trigger]')),
      changes: {
        count: document.querySelector('[data-session-changes]').dataset.sessionChanges,
        ...box(document.querySelector('[data-session-changes]')),
      },
      newTask: document.querySelectorAll('[data-session-header="phone"] [data-phone-new-task]')
        .length,
      horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1,
    }
  })
  assert.equal(facts.shell, 'phone')
  assert.equal(facts.workspace, 'sheet')
  assert.equal(facts.tabBar, 0)
  assert.equal(facts.strips, 0, 'no agent strip on the phone session page (规格 7.2)')
  assert.equal(facts.headerHeight, 56)
  assert.equal(facts.menu.label, '打开导航，3 个需要你')
  assert.ok(facts.menu.height >= 44 && facts.menu.width >= 44, JSON.stringify(facts.menu))
  assert.ok(facts.title.height >= 44, JSON.stringify(facts.title))
  assert.deepEqual(facts.changes, { count: '2', width: 44, height: 44 })
  assert.equal(facts.newTask, 0, 'with changes the right slot is the changes chip, not ✎')
  assert.equal(facts.horizontalScroll, false)
  await page.screenshot({ path: join(output, 'phone-session.png') })
  report.checks.push(
    'Phone session page: phone shell with the 56px top bar (☰ announcing 3 needing you, 44px title ▾, 44px changes chip “2” instead of ✎), no tab bar and no agent strip',
  )

  // 标题 ▾ → 会话菜单底部表单：会话树 · 侧聊 · 检查器 | 重命名 · 归档 | …，行高 48；返回键先关表单。
  await page.locator('[data-session-menu-trigger]').click()
  const menuSheet = page.locator('[data-session-menu-sheet]')
  await menuSheet.waitFor()
  const menuRows = await menuSheet.locator('button').evaluateAll((buttons) =>
    buttons.map((button) => ({
      text: button.innerText.trim().split('\n')[0],
      height: Math.round(button.getBoundingClientRect().height),
      marker:
        [...button.attributes].find(
          (attr) => attr.name.startsWith('data-session-') || attr.name === 'data-inspector-toggle',
        )?.name ?? '',
    })),
  )
  assert.ok(
    menuRows.every((entry) => entry.height >= 48),
    JSON.stringify(menuRows),
  )
  assert.deepEqual(
    menuRows.slice(0, 3).map((entry) => entry.marker),
    ['data-session-tree', 'data-session-side-chat', 'data-inspector-toggle'],
  )
  assert.ok(
    menuRows.some((entry) => entry.text === '重命名会话'),
    JSON.stringify(menuRows),
  )
  assert.ok(
    menuRows.some((entry) => entry.text === '归档'),
    JSON.stringify(menuRows),
  )
  await page.screenshot({ path: join(output, 'phone-session-menu.png') })
  assert.equal(await nativeBack(page), true)
  await menuSheet.waitFor({ state: 'detached' })
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  report.checks.push(
    'Phone title ▾ opens the session menu bottom sheet (会话树 · 侧聊 · 检查器 first, then 重命名 · 归档 · …, rows ≥48px); Android back closes it on the session page',
  )

  // 抽屉里一步处理普通批准（规格 7.2）：只发 1 次 POST，不离开当前会话；高风险行没有批准键。
  await page.locator('[data-session-header="phone"] [data-phone-menu]').click()
  const drawer = page.locator('[data-phone-drawer]')
  await drawer.waitFor()
  const standardActions = drawer.locator(
    '[data-sidebar-item="fx-standard"] [data-sidebar-drawer-actions]',
  )
  await standardActions.waitFor({ timeout: 20000 })
  assert.equal(
    await drawer.locator('[data-sidebar-item="fx-critical"] [data-sidebar-drawer-actions]').count(),
    0,
  )
  await standardActions.getByRole('button', { name: /允许/ }).click()
  for (let i = 0; i < 30 && fixture.posts.length < 1; i += 1) await page.waitForTimeout(100)
  await page.waitForTimeout(500)
  assert.deepEqual(
    fixture.posts.map((post) => [post.path, post.body]),
    [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
  )
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  assert.equal(await nativeBack(page), true, 'Android back closes the drawer first')
  await drawer.waitFor({ state: 'detached' })
  report.checks.push(
    'Phone drawer on the session page: the standard approval is handled in one step with exactly one POST without leaving the session; the critical row has no approve key; Android back closes the drawer first',
  )

  // 输入框芯片（规格 7.2）：盾牌与模型芯片 44px，点开各自的底部表单；返回键先关表单。
  const chips = await page.evaluate(() =>
    ['[data-composer-mode]', '.model-effort-pill'].map((selector) =>
      Math.round(document.querySelector(selector)?.getBoundingClientRect().height ?? 0),
    ),
  )
  assert.deepEqual(chips, [44, 44])
  for (const selector of ['[data-composer-mode]', '.model-effort-pill']) {
    await page.locator(selector).click()
    const sheet = page.locator('[data-popover-sheet]')
    await sheet.waitFor()
    assert.equal(await sheet.getAttribute('data-side'), 'bottom')
    if (selector === '[data-composer-mode]')
      await page.screenshot({ path: join(output, 'phone-mode-sheet.png') })
    assert.equal(await nativeBack(page), true)
    await sheet.waitFor({ state: 'detached' })
  }
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  report.checks.push(
    'Phone composer chips: the 44px shield and model chips open bottom sheets (not popovers); Android back closes each first',
  )

  // 改动芯片 → 检查器底部表单（改动 / 终端 / 文件 / 预览）；返回键先关表单，再回到全部代理。
  await page.locator('[data-session-changes]').click()
  const inspector = page.locator('[data-inspector-sheet]')
  await inspector.waitFor()
  await inspector.locator('[data-session-inspector="sheet"]').waitFor()
  const tabs = await inspector
    .locator('[data-inspector-tab-id]')
    .evaluateAll((nodes) =>
      nodes.map((node) => [
        node.dataset.inspectorTabId,
        Math.round(node.getBoundingClientRect().height),
      ]),
    )
  assert.deepEqual(
    tabs.slice(0, 4).map(([id]) => id),
    ['changes', 'terminal', 'files', 'preview'],
  )
  assert.ok(
    tabs.every(([, height]) => height >= 44),
    JSON.stringify(tabs),
  )
  await inspector.getByText('src/auth/login.ts').first().waitFor()
  await page.screenshot({ path: join(output, 'phone-changes-sheet.png') })
  // 关闭（Android 返回）：表单先以关闭态播退场动画，下一帧仍在 DOM 里（data-state=closed + data-sheet-leaving），
  // 退场结束后卸载（兜底超时 400ms），焦点回到“+N 改动”。
  const closeSheet = () =>
    page.evaluate(async () => {
      const backed = window.__pisperRequestBack?.()
      await new Promise((resolve) => requestAnimationFrame(() => resolve()))
      const sheet = document.querySelector('[data-inspector-sheet]')
      return {
        backed,
        state: sheet ? sheet.getAttribute('data-state') : null,
        leaving: Boolean(sheet?.hasAttribute('data-sheet-leaving')),
        at: performance.now(),
      }
    })
  const exit = await closeSheet()
  assert.deepEqual(
    { backed: exit.backed, state: exit.state, leaving: exit.leaving },
    { backed: true, state: 'closed', leaving: true },
    'the inspector sheet plays its exit transition before it unmounts',
  )
  await inspector.waitFor({ state: 'detached', timeout: 3000 })
  const exitMs = await page.evaluate((start) => performance.now() - start, exit.at)
  assert.ok(exitMs < 1000, `the inspector sheet unmounts after its exit transition (${exitMs}ms)`)
  assert.equal(
    await page.evaluate(() => document.activeElement?.hasAttribute('data-session-changes')),
    true,
    'focus returns to the +N 改动 trigger after the sheet closes',
  )
  // 退场途中重新打开（走同一个 show 入口；模态表单退场期间外部点按被 Radix 挡住，这里在同一段脚本里直接
  // 触发芯片的 click）：复用正在退场的同一个表单节点回到打开态，兜底超时过后也不会被卸载。
  await page.locator('[data-session-changes]').click()
  await inspector.locator('[data-session-inspector="sheet"]').waitFor()
  await inspector.evaluate((el) =>
    Promise.all(el.getAnimations().map((animation) => animation.finished.catch(() => {}))),
  )
  const reopen = await page.evaluate(async () => {
    window.__pisperRequestBack?.()
    await new Promise((resolve) => requestAnimationFrame(() => resolve()))
    const sheet = document.querySelector('[data-inspector-sheet]')
    const during = sheet?.getAttribute('data-state') ?? null
    if (sheet) sheet.dataset.smokeExiting = '1'
    document.querySelector('[data-session-changes]')?.click()
    return during
  })
  assert.equal(reopen, 'closed')
  await page.waitForTimeout(700)
  assert.deepEqual(
    await inspector.evaluate((el) => ({
      state: el.getAttribute('data-state'),
      leaving: el.hasAttribute('data-sheet-leaving'),
      reused: el.dataset.smokeExiting === '1',
    })),
    { state: 'open', leaving: false, reused: true },
    'reopening during the exit keeps the same sheet open past the fallback timeout',
  )
  assert.equal(await nativeBack(page), true)
  await inspector.waitFor({ state: 'detached', timeout: 3000 })
  // 减少动效：不播退场，关闭的下一帧表单已经不在 DOM 里。
  await page.emulateMedia({ reducedMotion: 'reduce' })
  await page.locator('[data-session-changes]').click()
  await inspector.locator('[data-session-inspector="sheet"]').waitFor()
  const reducedExit = await closeSheet()
  assert.deepEqual(
    { backed: reducedExit.backed, state: reducedExit.state },
    { backed: true, state: null },
    'with reduced motion the inspector sheet unmounts immediately',
  )
  await page.emulateMedia({ reducedMotion: 'no-preference' })
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  assert.deepEqual(aborts, [], 'closing sheets never stops the run')
  assert.equal(await nativeBack(page), true)
  await page.waitForURL((url) => url.hash === '#/board')
  await page.locator('[data-phone-topbar]').waitFor()
  report.checks.push(
    'Phone changes chip opens the inspector bottom sheet with 44px 改动 / 终端 / 文件 / 预览 tabs; Android back closes the sheet first (it stays mounted in its closed state for the exit transition, unmounts after it, and focus returns to the chip; reopening during the exit keeps it open; with reduced motion it unmounts at once), the next back returns to 全部代理, and nothing stops the run',
  )
  await context.close()
}

async function verifyTablet(boardSessions) {
  await resetLastSession()
  const { context } = await newContext(
    {
      viewport: { width: 1024, height: 768 },
      hasTouch: true,
      isMobile: true,
      colorScheme: 'light',
    },
    { mobileApp: true, sessions: boardSessions },
  )
  const page = await openPage(context, 'tablet', '#/board')
  await page.locator('[data-board-card]').first().waitFor({ timeout: 60000 })
  await settle(page)
  const board = await page.evaluate(() => ({
    phoneShell: document.documentElement.hasAttribute('data-phone-shell'),
    sidebar: document.querySelector('[data-shell-sidebar]')?.dataset.shellSidebar ?? null,
    titlebar: Boolean(document.querySelector('[data-app-titlebar]')),
    tabBar: Boolean(document.querySelector('[data-testid="mobile-tab-bar"]')),
    entries: [...document.querySelectorAll('[data-shell-sidebar] [data-sidebar-entry]')].map(
      (entry) => entry.dataset.sidebarEntry,
    ),
    // 平板（手机 App ≥960×640）用桌面的全部代理（A3 取代泳道）：分组列表，没有泳道。
    sections: [...document.querySelectorAll('[data-overview-section]')].map(
      (section) => section.dataset.overviewSection,
    ),
    lanes: document.querySelectorAll('[data-board-lane]').length,
    windowControls: document.querySelectorAll('.desktop-window-controls').length,
    horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1,
  }))
  assert.deepEqual(board, {
    phoneShell: false,
    sidebar: 'expanded',
    titlebar: false,
    tabBar: false,
    entries: ['newTask', 'search', 'board', 'features', 'settings'],
    sections: ['attention', 'running', 'done'],
    lanes: 0,
    windowControls: 0,
    horizontalScroll: false,
  })
  await page.screenshot({ path: join(output, 'tablet-board.png') })
  await page.goto(app.url + `/#/board/${BOARD_SESSION_ID}`)
  const currentRow = page.locator(
    `[data-shell-sidebar] [data-sidebar-row="${BOARD_SESSION_ID}"][aria-current="page"]`,
  )
  await currentRow.waitFor({ timeout: 30000 })
  const workspaceMode = await page
    .locator('[data-session-workspace]')
    .getAttribute('data-session-workspace')
  assert.equal(workspaceMode, 'overlay', 'the tablet session inspector is always an overlay')
  assert.equal(await page.locator('[data-session-header="phone"]').count(), 0)
  assert.equal(await page.locator('[data-agent-strip], [data-lane-strip]').count(), 0)
  // 触屏（pointer: coarse）：侧栏会话行与入口抬到 44px；悬停才出现的小号快捷处理与 ⋯ 不出现在触屏上
  // （触屏没有悬停，26px 的键也达不到 44px），点行进会话处理，不会误触批准。
  const touch = await page.evaluate(() => ({
    coarse: matchMedia('(pointer: coarse)').matches,
    rows: [...document.querySelectorAll('[data-shell-sidebar] [data-sidebar-row]')].map((row) =>
      Math.round(row.getBoundingClientRect().height),
    ),
    entries: [...document.querySelectorAll('[data-shell-sidebar] [data-sidebar-entry]')].map(
      (entry) => Math.round(entry.getBoundingClientRect().height),
    ),
    quickButtons: [...document.querySelectorAll('[data-sidebar-item] button')].filter(
      (button) => button.getBoundingClientRect().width > 0,
    ).length,
    horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1,
  }))
  assert.equal(touch.coarse, true)
  assert.ok(touch.rows.length > 0, `the tablet sidebar lists sessions ${JSON.stringify(touch)}`)
  assert.ok(
    touch.rows.every((height) => height >= 44),
    `touch rows ≥ 44 ${JSON.stringify(touch)}`,
  )
  assert.ok(
    touch.entries.every((height) => height >= 44),
    `touch entries ≥ 44 ${JSON.stringify(touch)}`,
  )
  assert.equal(touch.quickButtons, 0, 'no hover-only quick actions on touch')
  assert.equal(touch.horizontalScroll, false)
  const inspector = page.locator('[data-session-inspector]')
  if (!(await inspector.count())) await page.locator('[data-inspector-toggle]').click()
  await inspector.waitFor()
  assert.equal(await inspector.getAttribute('data-session-inspector'), 'overlay')
  await page.screenshot({ path: join(output, 'tablet-session.png') })
  assert.equal(await nativeBack(page), true, 'Android back closes the overlay inspector first')
  await inspector.waitFor({ state: 'detached' })
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  report.checks.push(
    'Tablet 1024x768 (phone app): desktop layout with the expanded 268px sidebar (five entries, no window controls, no title bar) and no tab bar; 全部代理 is the grouped list (需要你 · 运行中 · 今天完成, no lanes); on the session page the current session row is lit, sidebar rows and entries are at least 44px on touch and hover-only quick actions never show there; the session inspector is an overlay that Android back closes first',
  )
  await context.close()
}

// 冷启动快捷键（桌面与手机）：全新页面，外壳第一次画出来就按一次，必须生效且只生效一次。
// ⌘1（全部代理）只压入一条历史（idx 0 → 1）；桌面在“更多”页按 ⌘N（那里没有视图认领）只打开一个派发浮层。
// 手机主页上的 ⌘N 在 verifyPhoneShell 开头验证（聚焦草稿、不开底部表单）。
async function verifyColdStartShortcuts(boardSessions) {
  const delays = []
  for (const [label, options, extra, selector] of [
    ['phone', phoneViewport, { mobileApp: true }, '[data-new-task-draft="phone"]'],
    [
      'desktop',
      { viewport: { width: 1440, height: 900 }, colorScheme: 'light' },
      {},
      '[data-shell-sidebar]',
    ],
  ]) {
    await resetLastSession()
    const { context } = await newContext(options, { ...extra, sessions: boardSessions })
    await context.addInitScript(firstPaintProbe, selector)
    const page = await openPage(context, `cold ${label} ⌘1`, '#/')
    delays.push(`${label} ⌘1 ${await pressAfterFirstPaint(page, `${MOD_KEY}+Digit1`)}ms`)
    await page.waitForURL((url) => url.hash === '#/board', { timeout: 5000 })
    await page.locator('[data-board-root]').waitFor({ timeout: 30000 })
    assert.equal(await page.evaluate(() => window.history.state?.idx), 1, `${label}: one push`)
    assert.deepEqual(label === 'phone' ? await drawerCurrent(page) : await activeTabs(page), [
      'board',
    ])
    if (label === 'desktop') {
      await page.goto(app.url + '/#/more')
      await page.reload()
      delays.push(`desktop ⌘N ${await pressAfterFirstPaint(page, `${MOD_KEY}+KeyN`)}ms`)
      const overlay = page.locator('[data-launch-console="overlay"]')
      await overlay.waitFor({ timeout: 5000 })
      await page.waitForTimeout(400)
      assert.equal(await overlay.count(), 1, 'one ⌘N opens exactly one dispatch overlay')
      assert.equal(await page.getByRole('dialog').count(), 1)
      assert.equal(hashOf(page), '#/more')
    }
    await context.close()
  }
  report.checks.push(
    `Cold start: a single shortcut pressed right after the shell's first paint takes effect exactly once (${delays.join(', ')} after first paint) — ⌘1 pushes one /board entry on phone and desktop, desktop ⌘N on 更多 opens one dispatch overlay`,
  )
}

async function verifyNarrowDesktop(boardSessions) {
  for (const [width, height] of [
    [1080, 680],
    [640, 760],
    [390, 844],
  ]) {
    const label = `narrow desktop ${width}`
    const { context } = await newContext(
      { viewport: { width, height }, colorScheme: 'light' },
      { bridge: desktopBridgeScript, sessions: boardSessions },
    )
    const page = await openPage(context, label, '#/board')
    await page.locator('[data-shell-sidebar]').waitFor({ timeout: 60000 })
    await page.locator('[data-board-root]').waitFor({ timeout: 30000 })
    await settle(page, 600)
    const audit = await page.evaluate(() => {
      const controls = document.querySelector('.desktop-window-controls')
      const reservedLeft = window.innerWidth - 144
      // 没有标题栏了：核对侧栏与主区顶栏（看板页头）里的控件。
      const interactive = [
        ...document.querySelectorAll(
          '[data-shell-sidebar] :is(button, a[href]), [data-board-root] header :is(button, a[href], input, [role="button"], [role="tab"])',
        ),
      ]
        .filter((element) => !element.closest('.desktop-window-controls'))
        .map((element) => ({ element, box: element.getBoundingClientRect() }))
        .filter(({ box }) => box.width > 0 && box.height > 0)
        // 侧栏行的快捷处理与 ⋯ 只在悬停 / 聚焦时出现，平时是 visibility: hidden，不算被遮挡。
        .filter(({ element }) => getComputedStyle(element).visibility !== 'hidden')
        // 分组区单独滚动（品牌行、入口与底部设置固定）：滚出可视区的行不算被遮挡。
        .filter(({ element, box }) => {
          const scroller = element.closest('[data-sidebar-scroll]')?.getBoundingClientRect()
          const middle = box.top + box.height / 2
          return !scroller || (middle >= scroller.top && middle <= scroller.bottom)
        })
      const name = (element) =>
        (element.getAttribute('aria-label') || element.textContent || element.tagName)
          .trim()
          .slice(0, 30)
      // 遮挡：每个控件中心点命中的元素必须是它自己或它的后代。
      const covered = interactive
        .filter(({ element, box }) => {
          const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2)
          return !hit || !(element === hit || element.contains(hit))
        })
        .map(({ element }) => name(element))
      return {
        phoneShell: document.documentElement.hasAttribute('data-phone-shell'),
        tabBar: document.querySelectorAll('[data-testid="mobile-tab-bar"]').length,
        controls: controls ? Math.round(controls.getBoundingClientRect().width) : 0,
        intrusions: interactive
          // 窗口按钮只占右上角 48px 高：泳道标题等更靠下的控件不算。
          .filter(({ box }) => box.top < 48 && box.right > reservedLeft + 0.5)
          .map(({ element }) => name(element)),
        covered,
        entries: document.querySelectorAll('[data-shell-sidebar] [data-sidebar-entry]').length,
      }
    })
    assert.equal(audit.phoneShell, false, `${label}: a desktop window never enters the phone shell`)
    assert.equal(audit.tabBar, 0)
    assert.ok(audit.controls > 0, `${label}: window controls are visible`)
    assert.equal(audit.entries, 5)
    // 遮挡只核对 1080（Tauri 最小窗口）：侧栏常驻 268 之后，640 与 390 宽的桌面窗口并不存在，
    // 这两档只验证门控（永不进手机外壳）。
    if (width >= 1080) {
      assert.deepEqual(
        audit.intrusions,
        [],
        `${label}: nothing enters the 144px window-control corner`,
      )
      assert.deepEqual(audit.covered, [], `${label}: no sidebar or header control is covered`)
    }
    await page.screenshot({ path: join(output, `narrow-desktop-${width}.png`) })
    await page.goto(app.url + `/#/board/${BOARD_SESSION_ID}`)
    await page.locator('[data-session-shell]').waitFor({ timeout: 30000 })
    assert.equal(
      await page.locator('[data-session-shell]').getAttribute('data-session-shell'),
      'desktop',
    )
    assert.equal(await page.locator('[data-session-header="phone"]').count(), 0)
    assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
    await context.close()
  }
  report.checks.push(
    'Desktop windows (pisperDesktop + customTitlebar, 1080, 640 and 390 wide): the sidebar shell, never the phone shell or tab bar, on the board and the session page, window controls visible; at the 1080 minimum nothing enters the 144px corner and no sidebar or header control is covered',
  )
}

try {
  const executablePath = SMOKE_BROWSER || chromium.executablePath()
  assert(
    existsSync(executablePath),
    'Install Chromium with npx playwright-core install chromium, or set PISPER_SMOKE_BROWSER.',
  )
  browser = await chromium.launch({ headless: true, executablePath })
  await verifyComponentFixture(browser)

  await startRuntime()
  const realList = await fetch(app.url + '/api/sessions').then((response) => response.json())
  const realItem = (realList.sessions || []).find((session) => session.id === BOARD_SESSION_ID)
  assert.ok(realItem, 'the isolated runtime lists the fixture session')
  const boardSessions = [
    ...boardFixture().sessions,
    { ...realItem, unread: false, needsAttention: false, streaming: true },
  ]
  await verifyPhoneShell(boardSessions)
  await verifyColdStartShortcuts(boardSessions)
  await verifyPhoneSession(boardSessions)
  await verifyTablet(boardSessions)
  await verifyNarrowDesktop(boardSessions)
  assert.deepEqual(report.pageErrors, [])
  report.status = 'passed'
} catch (error) {
  report.status = 'failed'
  report.error = error?.stack || String(error)
  await failurePage?.screenshot({ path: join(output, 'failure.png') }).catch(() => {})
  throw error
} finally {
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2))
  await browser?.close()
  await app?.close?.()
  await new Promise((done) => (fixtureServer.listening ? fixtureServer.close(done) : done()))
  console.log(JSON.stringify({ status: report.status, checks: report.checks }, null, 2))
  console.log(`Mobile shell smoke report and screenshots: ${output}`)
}
if (report.status === 'passed')
  console.log(
    'PASS mobile shell: new-project and picker components, inspector sheet changes/terminal, phone home/board/more/components/settings, phone session sheets and back key, tablet layout, narrowed desktop gate',
  )
