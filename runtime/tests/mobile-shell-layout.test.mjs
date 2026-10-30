// 第 4 期手机外壳（规格 2.7、7.6、7.7）：phoneShell = 移动布局 && 不是桌面壳 && 不是手机 App 平板。
// 窄化的桌面窗口永远不进手机外壳（Tauri 最小窗口 1080，浏览器里也要求 !window.pisperDesktop）；
// 手机 App 在 ≥960×640 的视口里走桌面布局。手机专属界面只认这一份判断。
// 旧的三栏手机外壳（MobileThreePane / mobile-shell-layout.ts）已停止挂载，源码保留到第 6 期删除
// （需负责人同意）；文件末尾三条守卫只保护这份休眠代码本身，删除模块时随之删除。
import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
import {
  resolveMobileShellMode,
  moveMobileShellPane,
  swipeDeltaToDirection,
} from '../../src/components/layout/mobile-shell-layout.ts'
import { TABLET_VIEWPORT_QUERY } from '../../src/hooks/use-mobile.ts'

const compile = async (file) =>
  transformSync(await readFile(file, 'utf8'), { loader: 'ts', format: 'cjs' }).code

// 只支持这里用到的 (min|max)-(width|height) 与 and 组合；遇到别的写法直接报错，避免静默误判。
function mediaMatches(query, { width, height }) {
  return query.split(/\s+and\s+/).every((part) => {
    const match = part.trim().match(/^\((min|max)-(width|height):\s*(\d+)px\)$/)
    assert.ok(match, `unsupported media query part ${part}`)
    const value = match[2] === 'width' ? width : height
    return match[1] === 'min' ? value >= Number(match[3]) : value <= Number(match[3])
  })
}

const [useMobileCode, phoneShellCode] = await Promise.all([
  compile('src/hooks/use-mobile.ts'),
  compile('src/hooks/usePhoneShell.ts'),
])

// 在隔离上下文里求值真实的 use-mobile 与 usePhoneShell：媒体查询按给定视口计算，
// 客户端形态与桌面桥接按场景注入。返回首帧同步读取的结果（两者都在首帧同步取值）。
function shellFor({ width, height, mobileApp = false, desktop = false }) {
  const window = {
    matchMedia: (query) => ({
      matches: mediaMatches(query, { width, height }),
      addEventListener() {},
      removeEventListener() {},
    }),
    ...(desktop ? { pisperDesktop: { customTitlebar: true } } : {}),
  }
  const react = {
    useState: (initial) => [typeof initial === 'function' ? initial() : initial, () => {}],
    useEffect: () => {},
  }
  const load = (code, dependencies) => {
    const module = { exports: {} }
    runInNewContext(code, {
      module,
      exports: module.exports,
      window,
      require: (id) => {
        if (!(id in dependencies)) throw new Error(`unexpected dependency ${id}`)
        return dependencies[id]
      },
    })
    return module.exports
  }
  const useMobile = load(useMobileCode, { react })
  const shell = load(phoneShellCode, {
    '@/hooks/use-mobile': useMobile,
    '@/stores/client-store': {
      useClientStore: (selector) => selector({ client: mobileApp ? 'mobile-app' : 'web' }),
    },
  })
  return { phoneShell: shell.usePhoneShell(), tabletApp: shell.useTabletApp() }
}

test('the phone shell is for phones only: never a desktop shell window, never a phone-app tablet', () => {
  const cases = [
    // [场景, 视口与形态, 进手机外壳, 手机 App 平板]
    ['browser phone', { width: 390, height: 844 }, true, false],
    ['browser phone at the breakpoint', { width: 650, height: 900 }, true, false],
    ['browser just above the phone breakpoint', { width: 651, height: 900 }, false, false],
    ['browser tablet-sized window', { width: 1024, height: 768 }, false, false],
    ['browser desktop', { width: 1440, height: 900 }, false, false],
    // 窄化的桌面窗口（含被拖到手机宽度的情况）：标题栏保留，永远不换成底部标签栏。
    ['desktop shell narrowed to phone width', { width: 390, height: 844, desktop: true }, false],
    ['desktop shell at the minimum window', { width: 1080, height: 680, desktop: true }, false],
    ['phone app portrait', { width: 390, height: 844, mobileApp: true }, true, false],
    ['phone app landscape', { width: 844, height: 390, mobileApp: true }, true, false],
    ['phone app just under tablet width', { width: 959, height: 768, mobileApp: true }, true],
    ['phone app just under tablet height', { width: 1024, height: 639, mobileApp: true }, true],
    ['phone app tablet threshold', { width: 960, height: 640, mobileApp: true }, false, true],
    ['phone app tablet', { width: 1024, height: 768, mobileApp: true }, false, true],
    ['phone app large tablet', { width: 1366, height: 1024, mobileApp: true }, false, true],
  ]
  for (const [name, environment, phoneShell, tabletApp = false] of cases)
    assert.deepEqual(shellFor(environment), { phoneShell, tabletApp }, name)
})

test('the tablet boundary is one 960×640 rule shared by the shell, the session page and the retired pane model', async () => {
  assert.equal(TABLET_VIEWPORT_QUERY, '(min-width: 960px) and (min-height: 640px)')
  // 会话页为了不把 use-mobile 拉进会话块自带一份常量，必须与外壳的判断一致。
  const session = await readFile('src/app/SessionShell.tsx', 'utf8')
  assert.equal(
    session.match(/export const SESSION_TABLET_QUERY = '([^']+)'/)?.[1],
    TABLET_VIEWPORT_QUERY,
  )
  // 平板阈值沿用休眠三栏外壳的 pad 边界：同一组尺寸给出同样的结论。
  for (const [width, height] of [
    [390, 844],
    [844, 390],
    [959, 768],
    [1024, 639],
    [960, 640],
    [1024, 768],
    [1366, 1024],
  ])
    assert.equal(
      shellFor({ width, height, mobileApp: true }).tabletApp,
      resolveMobileShellMode(width, height) === 'pad',
      `${width}×${height}`,
    )
})

test('the app shell derives the phone shell from the same inputs as the shared hook', async () => {
  const app = await readFile('src/App.tsx', 'utf8')
  assert.match(app, /const mobileLayout = mobileApp \|\| phoneViewport/)
  assert.match(app, /const tabletApp = useIsTabletViewport\(\) && mobileApp/)
  assert.match(app, /const phoneShell = mobileLayout && !window\.pisperDesktop && !tabletApp/)
  // 根节点标记供经 Portal 挂到 body 的底部表单适配手机外壳；标题栏与底部标签栏二选一。
  assert.match(app, /document\.documentElement\.toggleAttribute\('data-phone-shell', phoneShell\)/)
  assert.match(app, /\{!phoneShell && \(\s*<AppTitlebar\b/)
  assert.match(app, /\{phoneShell && \(\s*<Suspense fallback=\{tabBarFallback\}>\s*<ShellTabBar\b/)
  // 平板仍按移动端处理原生返回（BackNavigation）与软键盘，只是布局换成桌面。
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
})

test('phone-only surfaces read the shared phone-shell gate instead of a viewport check', async () => {
  // 手机看板、手机会话页（代理条 / 顶栏看板入口）、更多里的组件磁贴、组件列表 → 详情、主页草稿
  // 与会话审批的底部确认表单：都只在 usePhoneShell() 为真时出现。
  const gated = {
    'src/features/board/BoardPage.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/SessionShell.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/BoardEntry.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/MorePage.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/SessionHome.tsx': /const phone = usePhoneShell\(\)/,
    'src/features/custom-ui/CustomUiPage.tsx': /const phone = usePhoneShell\(\) && !embedded/,
    'src/features/chat/ToolApproval.tsx': /const phone = usePhoneShell\(\)/,
  }
  for (const [file, pattern] of Object.entries(gated))
    assert.match(await readFile(file, 'utf8'), pattern, file)
  const board = await readFile('src/features/board/BoardPage.tsx', 'utf8')
  assert.match(
    board,
    /<BoardView key=\{phone \? 'phone' : 'desktop'\} \{\.\.\.props\} phone=\{phone\} \/>/,
  )
  assert.match(
    await readFile('src/features/board/usePhoneShell.ts', 'utf8'),
    /export \{ usePhoneShell \} from '@\/hooks\/usePhoneShell'/,
  )
  // 手机外壳的门控只在两处定义（共享 Hook 与应用壳）；其余代码不得自己拼“移动布局且不是桌面壳”。
  // FocusSession 里的旧任务顶栏只在非路由的旧入口出现，随第 6 期删除。
  const allowed = new Set([
    'src/App.tsx',
    'src/hooks/usePhoneShell.ts',
    'src/features/chat/FocusSession.tsx',
  ])
  const sources = async (directory) =>
    (
      await Promise.all(
        (await readdir(directory, { withFileTypes: true })).map((entry) => {
          const target = path.join(directory, entry.name)
          if (entry.isDirectory()) return sources(target)
          return /\.(?:ts|tsx)$/.test(entry.name) ? [target] : []
        }),
      )
    ).flat()
  for (const file of await sources('src')) {
    const name = file.replaceAll('\\', '/')
    // 只看“裸桥接取反 + 手机 / 移动条件”写在同一行的门控；桌面专属能力（?.terminalCreate 等）不算。
    const gates = (await readFile(file, 'utf8'))
      .split('\n')
      .filter((line) => /!window\.pisperDesktop(?![?.\w])/.test(line) && /mobile|phone/i.test(line))
    if (gates.length) assert.ok(allowed.has(name), `${name} derives its own phone-shell gate`)
  }
})

test('every phone approval entry keeps critical approvals behind the hold-to-confirm sheet', async () => {
  const [board, card, toolApproval, confirm, tabBar, header, entry] = await Promise.all(
    [
      'src/features/board/PhoneBoard.tsx',
      'src/features/board/AgentCard.tsx',
      'src/features/chat/ToolApproval.tsx',
      'src/features/board/ApprovalConfirm.tsx',
      'src/app/ShellTabBar.tsx',
      'src/features/chat/SessionPhoneHeader.tsx',
      'src/app/BoardEntry.tsx',
    ].map((file) => readFile(file, 'utf8')),
  )
  // 手机看板：卡片按钮只把动作交给看板控制器（critical 在那里只打开确认层，见 board-ui-model.test），
  // 确认层是同一个按住 0.6 秒的底部表单，按住结束才调 confirmApproval。
  assert.match(card, /onClick=\{\(\) => handlers\.onApprove\(item, approval\)\}/)
  assert.match(card, /aria-haspopup=\{critical \? 'dialog' : undefined\}/)
  assert.match(board, /handlers=\{controller\.handlers\}/)
  assert.match(
    board,
    /<ApprovalConfirm[\s\S]{0,120}\bphone\b[\s\S]{0,200}onConfirm=\{\(\) => void controller\.confirmApproval\(\)\}/,
  )
  // 手机会话页的审批面板：critical 的“允许…”只打开确认层（底部表单形态）。
  assert.match(
    toolApproval,
    /onClick=\{\(\) => \(critical \? setConfirming\(true\) : resolve\(true\)\)\}/,
  )
  assert.match(toolApproval, /<ApprovalConfirm[\s\S]{0,300}phone=\{phone\}/)
  // 代理条操作表单（PhoneAgentStrip）的同一约束见 phone-session-page.test。
  // 确认层：手机是底部表单；按住 600ms 才确认，点击不能绕过，详情未到时不能按住。
  assert.match(confirm, /export const APPROVAL_HOLD_MS = 600/)
  assert.match(
    confirm,
    /if \(phone\)\s*return \(\s*<Sheet open=\{open\} onOpenChange=\{onOpenChange\}>/,
  )
  assert.match(confirm, /onClick=\{\(event\) => event\.preventDefault\(\)\}/)
  assert.match(confirm, /disabled=\{pending \|\| !approval\}/)
  // 标签栏、手机顶栏与看板入口只显示计数或跳转，从不批准。
  for (const [file, source] of [
    ['ShellTabBar', tabBar],
    ['SessionPhoneHeader', header],
    ['BoardEntry', entry],
  ])
    assert.doesNotMatch(
      source,
      /\.approve\(|approveBoardApproval|confirmCritical|resolve\(true\)/,
      file,
    )
  for (const [file, source] of [
    ['PhoneBoard', board],
    ['AgentCard', card],
  ])
    assert.doesNotMatch(source, /approveBoardApproval|confirmCritical|\.approve\(/, file)
})

// —— 休眠的三栏手机外壳（第 6 期删除，需负责人同意）——

test('dormant pane model: compact and short landscape viewports stay on one pane', () => {
  for (const [width, height] of [
    [390, 844],
    [844, 390],
    [959, 768],
    [1024, 639],
  ]) {
    assert.equal(resolveMobileShellMode(width, height), 'phone')
  }
  for (const [width, height] of [
    [960, 640],
    [1024, 768],
    [1366, 1024],
  ]) {
    assert.equal(resolveMobileShellMode(width, height), 'pad')
  }
})

test('dormant pane model: horizontal navigation traverses sessions, conversation and workspace without wrapping', () => {
  assert.equal(moveMobileShellPane('chat', -1), 'sessions')
  assert.equal(moveMobileShellPane('chat', 1), 'context')
  assert.equal(moveMobileShellPane('sessions', 1), 'chat')
  assert.equal(moveMobileShellPane('context', -1), 'chat')
  assert.equal(moveMobileShellPane('sessions', -1), 'sessions')
  assert.equal(moveMobileShellPane('context', 1), 'context')
})

test('dormant pane model: vertical scrolling, short gestures and ambiguous diagonals never switch panes', () => {
  for (const [dx, dy] of [
    [0, 100],
    [47, 0],
    [-47, 0],
    [90, 100],
    [-100, -90],
  ]) {
    assert.equal(swipeDeltaToDirection(dx, dy), 0)
  }
  assert.equal(swipeDeltaToDirection(-48, 0), 1)
  assert.equal(swipeDeltaToDirection(48, 0), -1)
  assert.equal(swipeDeltaToDirection(-120, 50), 1)
  assert.equal(swipeDeltaToDirection(120, -50), -1)
})
