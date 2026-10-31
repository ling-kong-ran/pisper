// 手机外壳冒烟（第 4 期重写）。两部分，都只用本机浏览器、不启动模型、不读用户配置：
// 1. 组件夹具（scripts/fixtures/mobile-shell.jsx）：新外壳仍在使用的真实组件——看板“新建项目…”、
//    工作目录选择器（含 Android SAF 导入）、附件的系统选择器、检查器底部表单里的改动与终端。
// 2. 真实生产构建（dist）+ 进程内隔离 Runtime（临时数据目录、随机端口）；看板、/live 与审批在浏览器
//    边界用 scripts/fixtures/board-fixture.mjs 固定，会话页的转录来自写进临时目录的真实会话：
//    - 390×844 手机 App：主页（会话）草稿与返回根、底部标签栏（会话 · 看板 · [新任务] · 更多 · 设置）
//      与看板徽标、安全区、软键盘收起标签栏、看板跳转条与区段、审批卡（standard 一次允许只发 1 次 POST；
//      critical 只开按住确认表单）、派发底部表单、更多磁贴与角标、组件列表 → 详情、设置；
//    - 手机会话页：隐藏标签栏、顶栏看板入口、代理条操作表单、“+N 改动”检查器底部表单、
//      Android 返回键先关表单再回上一级；
//    - 平板 1024×768（手机 App）：桌面布局、泳道收缩规则、检查器覆盖层；
//    - 窄宽度桌面模拟（pisperDesktop + customTitlebar）：不进手机外壳，标题栏无遮挡。
//    - 冷启动快捷键（手机与 1440 桌面）：外壳第一次画出来就按一次 ⌘N / ⌘1，生效且只生效一次；
//      手机看板上“会话主页”键紧接 ⌘N，请求等主页提交后交给草稿（快捷键 v2 里会话主页默认未绑定，
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
// 一个已安装的旧版 iframe 组件：组件列表、详情与更多页都能看到真实条目。
const componentDir = join(dataDir, 'custom-ui', 'game-asset-demo')
await mkdir(componentDir, { recursive: true })
await writeFile(
  join(componentDir, 'manifest.json'),
  JSON.stringify({
    id: 'game-asset-demo',
    name: '游戏素材工作台',
    version: '1.2.0',
    description: '旧版组件 · 沙箱运行',
    entry: 'index.html',
    permissions: ['notify'],
  }),
)
await writeFile(
  join(componentDir, 'index.html'),
  '<!doctype html><meta charset="utf-8"><body>fixture component</body>',
)

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
    const response = await route.fetch()
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
// 点亮的一级入口：手机标签栏的标签，或桌面侧栏的入口（全部代理 → board、功能 → more、设置 → settings）。
const activeTabs = (page) =>
  page.evaluate(() =>
    [
      ...document.querySelectorAll(
        '[data-shell-tab][aria-current="page"], [data-shell-sidebar] [data-sidebar-entry][aria-current="page"]',
      ),
    ].map(
      (tab) =>
        tab.dataset.shellTab ||
        { board: 'board', features: 'more', settings: 'settings' }[tab.dataset.sidebarEntry],
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

// critical 卡片的审批详情来自 /live（错峰拉取）；详情到达前“允许…”不可点，先等它就绪。
async function waitForApprovals(page) {
  for (const id of ['fx-critical', 'fx-standard'])
    await page
      .locator(`[data-board-card="${id}"]`)
      .getByRole('button', { name: id === 'fx-critical' ? '允许…' : '允许', exact: true })
      .and(page.locator(':enabled'))
      .waitFor({ timeout: 20000 })
}

const phoneViewport = {
  viewport: { width: 390, height: 844 },
  isMobile: true,
  hasTouch: true,
  deviceScaleFactor: 2,
  colorScheme: 'light',
}

async function verifyPhoneShell(boardSessions) {
  await resetLastSession()
  const { context, fixture } = await newContext(phoneViewport, {
    mobileApp: true,
    safeArea: true,
    keyboard: true,
    sessions: boardSessions,
  })
  // 主页是会话：全新设备落在新任务草稿（手机会话外壳，不显示标签栏）；主页是返回的根。
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
  assert.equal(await page.getByTestId('mobile-tab-bar').count(), 0)
  const homeEntry = await page.locator('[data-board-entry]').boundingBox()
  assert.ok(homeEntry && homeEntry.height >= 43.5, `home board entry ${JSON.stringify(homeEntry)}`)
  assert.equal(await nativeBack(page), false, 'Android back on the home draft hands over')
  report.checks.push(
    `Phone home: a fresh device lands on the new-task draft in the phone session shell without the tab bar (draft not autofocused; a single hardware ⌘N pressed ${coldDelay}ms after the draft's first paint focuses it without opening the dispatch sheet); the 看板 entry is ≥44px; Android back on the home hands over to the platform`,
  )

  // 看板：标签栏、徽标、安全区、跳转条与区段、审批卡、派发底部表单、软键盘。
  await page.locator('[data-board-entry]').click()
  await page.waitForURL((url) => url.hash === '#/board')
  const tabBar = page.getByTestId('mobile-tab-bar')
  await tabBar.waitFor()
  await page.locator('[data-board-card]').first().waitFor({ timeout: 30000 })
  await waitForApprovals(page)
  assert.equal(
    await page.evaluate(() => document.documentElement.hasAttribute('data-phone-shell')),
    true,
  )
  assert.equal(await page.locator('[data-app-titlebar], [data-shell-sidebar]').count(), 0)
  const bar = await tabBar.evaluate((nav) => ({
    order: [...nav.children].map(
      (child) => child.dataset.shellTab || (child.querySelector('button') ? 'new' : ''),
    ),
    paddingBottom: getComputedStyle(nav).paddingBottom,
    tabs: [...nav.querySelectorAll('[data-shell-tab]')].map((tab) => {
      const box = tab.getBoundingClientRect()
      return { id: tab.dataset.shellTab, width: box.width, height: box.height }
    }),
    newTask: (() => {
      const button = nav.querySelector(':scope > div > button')
      const box = button.getBoundingClientRect()
      return { label: button.getAttribute('aria-label'), width: box.width, height: box.height }
    })(),
    bottomGap: window.innerHeight - nav.getBoundingClientRect().bottom,
  }))
  assert.deepEqual(bar.order, ['session', 'board', 'new', 'more', 'settings'])
  assert.deepEqual(bar.newTask, { label: '新任务', width: 52, height: 40 })
  for (const tab of bar.tabs)
    assert.ok(tab.height >= 56 && tab.width >= 44, `tab touch target ${JSON.stringify(tab)}`)
  assert.equal(bar.paddingBottom, '34px', 'the tab bar keeps clear of the bottom safe area')
  assert.ok(Math.abs(bar.bottomGap) < 1, `tab bar sits on the bottom edge ${bar.bottomGap}`)
  const boardTab = tabBar.locator('[data-shell-tab="board"]')
  await page.waitForFunction(
    () =>
      document
        .querySelector('[data-testid="mobile-tab-bar"] [data-shell-tab="board"]')
        ?.getAttribute('aria-label') === '看板, 3 项需要你',
  )
  assert.deepEqual(await activeTabs(page), ['board'])
  assert.equal((await boardTab.innerText()).replace(/\s+/g, ''), '3看板')
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'phone-board.png') })
  report.checks.push(
    'Phone tab bar: 会话 · 看板 · [新任务 52×40] · 更多 · 设置, 56px tabs, 34px bottom safe area, 看板 lit with the amber 3 badge, no title bar, no horizontal scroll',
  )

  const jumpBar = page.getByRole('navigation', { name: '跳到泳道', exact: true })
  const chips = await jumpBar
    .getByRole('button')
    .evaluateAll((buttons) =>
      buttons.map((button) => [button.textContent.trim(), button.getAttribute('aria-current')]),
    )
  assert.deepEqual(
    chips.map(([text]) => text.replace(/\d+$/, '')),
    ['需要你', '运行中', '已完成'],
  )
  assert.equal(chips[0][1], 'true', 'the attention chip starts selected')
  assert.deepEqual(
    await page
      .locator('[data-board-section]')
      .evaluateAll((sections) => sections.map((section) => section.dataset.boardSection)),
    ['attention', 'running', 'done'],
  )
  for (const box of await jumpBar
    .getByRole('button')
    .evaluateAll((buttons) => buttons.map((button) => button.getBoundingClientRect().height)))
    assert.ok(box >= 40, `jump chip height ${box}`)
  // 点“已完成”：滚到该区段（末尾区段较短时滚到底），选中留在“已完成”，平滑滚动结束后也不被前一个区段抢走。
  const doneSelected = () =>
    page.evaluate(() => {
      const scroller = document.querySelector('[data-board-sections]')
      const section = scroller?.querySelector('[data-board-section="done"]')
      const chip = [...document.querySelectorAll('nav[aria-label="跳到泳道"] button')].find(
        (button) => button.textContent.trim().startsWith('已完成'),
      )
      const offset = section.getBoundingClientRect().top - scroller.getBoundingClientRect().top
      const atBottom = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2
      return chip?.getAttribute('aria-current') === 'true' && (Math.abs(offset) < 16 || atBottom)
    })
  await jumpBar.getByRole('button', { name: /^已完成/ }).click()
  for (let i = 0; i < 40 && !(await doneSelected()); i += 1) await page.waitForTimeout(100)
  await page.waitForTimeout(600)
  assert.equal(await doneSelected(), true, 'tapping 已完成 keeps it selected after scrolling')
  await jumpBar.getByRole('button', { name: /^需要你/ }).click()
  await page.waitForFunction(() => document.querySelector('[data-board-sections]')?.scrollTop === 0)
  report.checks.push(
    'Phone board jump bar: 需要你 / 运行中 / 已完成 chips (queued hidden without the capability), attention selected first; tapping 已完成 scrolls its section to the top and selects it; sections follow the lane order',
  )

  // critical：“允许…”只打开按住确认表单；轻点按住键不批准；返回键先关表单，仍在看板。
  const critical = page.locator('[data-board-card="fx-critical"]')
  await critical.getByRole('button', { name: '允许…', exact: true }).click()
  const confirm = page.getByRole('dialog', { name: '确认高风险操作', exact: true })
  await confirm.waitFor()
  await confirm.locator('[data-approval-hold]').tap()
  await page.waitForTimeout(300)
  assert.equal(fixture.posts.length, 0, 'critical 允许… and a tap on the hold button never approve')
  assert.equal(await nativeBack(page), true)
  await confirm.waitFor({ state: 'detached' })
  assert.equal(hashOf(page), '#/board')
  assert.equal(fixture.posts.length, 0)
  // standard：一次点击只发 1 次 POST。
  const standardAllow = page
    .locator('[data-board-card="fx-standard"]')
    .getByRole('button', { name: '允许', exact: true })
  const allowBox = await standardAllow.boundingBox()
  assert.ok(allowBox && allowBox.height >= 44, `approve touch target ${JSON.stringify(allowBox)}`)
  await standardAllow.click()
  for (let i = 0; i < 30 && fixture.posts.length < 1; i += 1) await page.waitForTimeout(100)
  await page.waitForTimeout(500)
  assert.deepEqual(
    fixture.posts.map((post) => [post.path, post.body]),
    [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
  )
  assert.equal(hashOf(page), '#/board', 'approving inline stays on the board')
  report.checks.push(
    'Phone approval cards: critical 允许… only opens the hold-to-confirm sheet (a tap never approves; Android back closes it first and stays on the board); standard 允许 (≥44px) sends exactly one POST {approved:true}',
  )

  // 新任务：派发底部表单；返回键先关表单。
  await tabBar.getByRole('button', { name: '新任务', exact: true }).click()
  const dispatch = page.getByRole('dialog', { name: '新任务', exact: true })
  await dispatch.waitFor()
  await dispatch.locator('[data-launch-console="sheet"]').waitFor()
  // 输入框单独占一行（“在 [项目 ▾] 中”在上一行）：390 宽下不能被项目芯片挤到一百多像素宽。
  const sheetInput = await dispatch.locator('[data-launch-input]').boundingBox()
  assert.ok(sheetInput && sheetInput.width >= 300, `dispatch textarea width ${sheetInput?.width}`)
  assert.ok(sheetInput.height >= 44, `dispatch textarea height ${sheetInput.height}`)
  assert.equal(await nativeBack(page), true)
  await dispatch.waitFor({ state: 'detached' })
  assert.equal(hashOf(page), '#/board')
  // 软键盘弹起：外壳随可视视口收缩，标签栏让出高度；收起后回来。
  await page.evaluate(() => window.__setKeyboard(336))
  await page.waitForFunction(
    () => document.querySelector('.app-shell')?.dataset.mobileKeyboard === 'open',
  )
  assert.equal(await tabBar.isVisible(), false, 'the tab bar yields to the software keyboard')
  await page.evaluate(() => window.__setKeyboard(0))
  await page.waitForFunction(() => !document.querySelector('.app-shell')?.dataset.mobileKeyboard)
  assert.equal(await tabBar.isVisible(), true)
  report.checks.push(
    'Phone 新任务 opens the dispatch bottom sheet with a full-width (≥300px, ≥44px tall) input under the project row (Android back closes it first); the software keyboard hides the tab bar and it returns when the keyboard closes',
  )

  // 看板上“会话主页”键（改绑的 ⌘9）紧接 ⌘N：路由在 transition 里提交，按 ⌘N 时渲染的仍是看板（手机看板不认领，
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
  await page.locator('[data-board-entry]').click()
  await page.waitForURL((url) => url.hash === '#/board')
  await tabBar.waitFor()
  report.checks.push(
    'Phone race: the rebound session-home key then ⌘N at once from the board lands on the home draft and focuses it (the request waits for the home to commit instead of opening the dispatch sheet over the leaving board)',
  )

  // 更多：磁贴、角标、组件磁贴；组件列表 → 详情；返回键沿层级回到主页。
  await tabBar.locator('[data-shell-tab="more"]').click()
  await page.waitForURL((url) => url.hash === '#/more')
  await page.locator('[data-more-tile]').first().waitFor()
  await page.locator('[data-more-badge]').nth(3).waitFor({ timeout: 20000 })
  assert.deepEqual(await activeTabs(page), ['more'])
  assert.equal(
    await boardTab.getAttribute('aria-label'),
    '看板, 2 项需要你',
    'the badge follows the approval on every page',
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
  const componentsTile = page.locator('[data-more-tile="shell:components"]')
  await componentsTile.waitFor()
  // “组件”是普通入口磁贴，不能画成“+ 添加”那种虚线空位。
  assert.equal(
    await componentsTile.evaluate((node) => getComputedStyle(node).borderTopStyle),
    'solid',
  )
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'phone-more.png') })
  await componentsTile.click()
  await page.waitForURL((url) => url.hash === '#/components')
  const list = page.locator('[data-components-phone="list"]')
  await list.waitFor()
  assert.deepEqual(await activeTabs(page), ['more'], 'components opened from 更多 keep 更多 lit')
  const row = page.locator('[data-component-row="game-asset-demo"]')
  const rowBox = await row.boundingBox()
  assert.ok(rowBox && rowBox.height >= 44, `component row ${JSON.stringify(rowBox)}`)
  await row.click()
  await page.locator('[data-components-phone="detail"]').waitFor()
  assert.equal(hashOf(page), '#/components/game-asset-demo')
  // 手机页头：左对齐 24px 大标题与 44px 高的“‹ 更多”文字返回键，不放主题键。
  const phoneHeader = await page.evaluate(() => {
    const back = document.querySelector('[data-back-to-more]')
    const header = back?.closest('header')
    const title = header?.querySelector('h1')
    return {
      back: back ? Math.round(back.getBoundingClientRect().height) : 0,
      backText: back?.textContent?.trim(),
      titleSize: title ? getComputedStyle(title).fontSize : '',
      theme: [...(header?.querySelectorAll('[data-testid="theme-toggle"]') ?? [])].filter(
        (node) => node.getClientRects().length > 0,
      ).length,
    }
  })
  assert.deepEqual(phoneHeader, { back: 44, backText: '更多', titleSize: '24px', theme: 0 })
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
    'Phone 更多: tiles with 1 运行中 / 2 启用 / 1 断开 (amber) / 3 在线 badges and the 组件 tile; components list → detail (≥44px rows) with 更多 lit; Android back walks detail → list → 更多 → home, and the home hands over to the platform',
  )

  // 设置：手机分区导航，“独立组件”标签换成跳到组件页的链接；看板徽标仍在。
  await page.goto(app.url + '/#/config/interface')
  await page.locator('[data-components-moved]').waitFor({ timeout: 30000 })
  assert.equal(await page.locator('[data-mobile-navigation="settings"]').count(), 1)
  assert.equal(await page.getByRole('tab', { name: /独立组件/ }).count(), 0)
  assert.deepEqual(await activeTabs(page), ['settings'])
  assert.equal(
    await boardTab.getAttribute('aria-label'),
    '看板, 2 项需要你',
    'the badge follows the approval on every page',
  )
  assert.equal(await noHorizontalScroll(page), true)
  await page.screenshot({ path: join(output, 'phone-settings.png') })
  await page.locator('[data-components-moved]').click()
  await page.waitForURL((url) => url.hash === '#/components')
  report.checks.push(
    'Phone settings: mobile section navigation, the standalone-widgets tab is replaced by a link to /components, 设置 lit and the board badge kept',
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
  await page.getByTestId('mobile-tab-bar').waitFor({ timeout: 60000 })
  await waitForApprovals(page)
  await page.locator(`[data-board-card="${BOARD_SESSION_ID}"]`).click()
  await page.waitForURL((url) => url.hash === `#/board/${BOARD_SESSION_ID}`)
  const header = page.locator('[data-session-header="phone"]')
  await header.waitFor({ timeout: 30000 })
  await page.locator('[data-agent-chip="fx-critical"]').waitFor({ timeout: 30000 })
  await page.locator('[data-session-changes]').waitFor({ timeout: 30000 })
  const facts = await page.evaluate(() => {
    const entry = document.querySelector('[data-board-entry]')
    const box = entry.getBoundingClientRect()
    return {
      shell: document.querySelector('[data-session-shell]')?.dataset.sessionShell,
      workspace: document.querySelector('[data-session-workspace]')?.dataset.sessionWorkspace,
      tabBar: document.querySelectorAll('[data-testid="mobile-tab-bar"]').length,
      laneStrip: document.querySelectorAll('[data-lane-strip]').length,
      headerHeight: Math.round(
        document.querySelector('[data-session-header="phone"]').getBoundingClientRect().height,
      ),
      entry: { label: entry.getAttribute('aria-label'), height: box.height, width: box.width },
      chips: [...document.querySelectorAll('[data-agent-chip]')].map((chip) => ({
        id: chip.dataset.agentChip,
        lane: chip.dataset.lane,
        height: Math.round(chip.getBoundingClientRect().height),
      })),
      horizontalScroll: document.documentElement.scrollWidth > window.innerWidth + 1,
    }
  })
  assert.equal(facts.shell, 'phone')
  assert.equal(facts.workspace, 'sheet')
  assert.equal(facts.tabBar, 0, 'the session page hides the tab bar')
  assert.equal(facts.laneStrip, 0)
  assert.equal(facts.headerHeight, 56)
  assert.equal(facts.entry.label, '打开看板，3 项需要你')
  assert.ok(facts.entry.height >= 44 && facts.entry.width >= 44, JSON.stringify(facts.entry))
  assert.equal(facts.chips[0].lane, 'attention', 'attention chips come first')
  assert.ok(
    facts.chips.every((chip) => chip.height >= 44),
    JSON.stringify(facts.chips),
  )
  assert.equal(facts.horizontalScroll, false)
  await page.screenshot({ path: join(output, 'phone-session.png') })
  report.checks.push(
    'Phone session page: phone shell with the 56px top bar, no tab bar and no lane strip, the 看板 entry announces 3 items needing you (≥44px), 44px agent chips with attention first',
  )

  // 代理条 → critical 操作表单：只有“允许…”，进入按住确认表单；返回键先关表单，留在会话页。
  await page.locator('[data-agent-chip="fx-critical"]').click()
  const criticalSheet = page.locator('[data-agent-sheet="fx-critical"]')
  await criticalSheet.waitFor()
  await criticalSheet
    .locator('[data-agent-sheet-command]')
    .getByText('git push --force-with-lease origin fix/ci')
    .waitFor()
  assert.equal(
    await criticalSheet
      .locator('[data-agent-sheet-approve]')
      .getAttribute('data-agent-sheet-approve'),
    'critical',
  )
  assert.equal(
    (await criticalSheet.locator('[data-agent-sheet-approve]').innerText()).trim(),
    '允许…',
  )
  await criticalSheet.locator('[data-agent-sheet-approve]').click()
  const confirm = page.getByRole('dialog', { name: '确认高风险操作', exact: true })
  await confirm.waitFor()
  await confirm.locator('[data-approval-hold]').tap()
  await page.waitForTimeout(300)
  assert.equal(fixture.posts.length, 0, 'the agent sheet never approves critical in one step')
  assert.equal(await nativeBack(page), true)
  await confirm.waitFor({ state: 'detached' })
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  // standard：表单里一步允许，只发 1 次 POST，不离开当前会话。
  await page.locator('[data-agent-chip="fx-standard"]').click()
  const standardSheet = page.locator('[data-agent-sheet="fx-standard"]')
  await standardSheet.waitFor()
  await standardSheet.locator('[data-agent-sheet-approve="standard"]').click()
  await standardSheet.waitFor({ state: 'detached' })
  await page.waitForTimeout(500)
  assert.deepEqual(
    fixture.posts.map((post) => [post.path, post.body]),
    [['/api/sessions/fx-standard/approvals/approval-standard', { approved: true }]],
  )
  assert.equal(hashOf(page), `#/board/${BOARD_SESSION_ID}`)
  report.checks.push(
    'Phone agent strip: the critical chip sheet shows the full command and only 允许…, which opens the hold-to-confirm sheet (a tap never approves; Android back closes it on the session page); the standard chip approves with exactly one POST without leaving the session',
  )

  // “+N 改动 ›” → 检查器底部表单（改动 / 终端 / 文件 / 预览）；返回键先关表单，再回到看板。
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
  await page.getByTestId('mobile-tab-bar').waitFor()
  report.checks.push(
    'Phone +N 改动 › opens the inspector bottom sheet with 44px 改动 / 终端 / 文件 / 预览 tabs; Android back closes the sheet first (it stays mounted in its closed state for the exit transition, unmounts after it, and focus returns to +N 改动; reopening during the exit keeps it open; with reduced motion it unmounts at once), the next back returns to the board where the tab bar is back, and nothing stops the run',
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
    assert.deepEqual(await activeTabs(page), ['board'])
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
  const executablePath = process.env.PISPER_SMOKE_BROWSER || chromium.executablePath()
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
