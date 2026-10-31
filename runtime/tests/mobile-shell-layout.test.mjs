// 第 4 期手机外壳（规格 2.7、7.6、7.7）：phoneShell = 移动布局 && 不是桌面壳 && 不是手机 App 平板。
// 窄化的桌面窗口永远不进手机外壳（Tauri 最小窗口 1080，浏览器里也要求 !window.pisperDesktop）；
// 手机 App 在 ≥960×640 的视口里走桌面布局。手机专属界面只认这一份判断。
// 旧的三栏手机外壳（MobileThreePane / mobile-shell-layout.ts）在 A4 删除（规格 7.6，负责人已同意），
// 只保护那份休眠代码本身的三条守卫随之删除；平板阈值改为直接钉住尺寸表。
import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import { runInNewContext } from 'node:vm'
import { transformSync } from 'esbuild'
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
  // 平板阈值（≥960×640，规格 7.5）：手机竖横屏与差一点的尺寸都还是手机外壳。
  for (const [width, height, tablet] of [
    [390, 844, false],
    [844, 390, false],
    [959, 768, false],
    [1024, 639, false],
    [960, 640, true],
    [1024, 768, true],
    [1366, 1024, true],
  ])
    assert.equal(
      shellFor({ width, height, mobileApp: true }).tabletApp,
      tablet,
      `${width}×${height}`,
    )
})

test('the app shell derives the phone shell from the same inputs as the shared hook', async () => {
  const app = await readFile('src/App.tsx', 'utf8')
  assert.match(app, /const mobileLayout = mobileApp \|\| phoneViewport/)
  assert.match(app, /const tabletApp = useIsTabletViewport\(\) && mobileApp/)
  assert.match(app, /const phoneShell = mobileLayout && !window\.pisperDesktop && !tabletApp/)
  // 根节点标记供经 Portal 挂到 body 的底部表单适配手机外壳；侧栏外壳与手机抽屉二选一（规格 v2 第 7.1 节），
  // 手机外壳在会话页与主页以外的页面顶上加一条 56px 顶栏（☰ · 标题 · ✎）。
  assert.match(app, /document\.documentElement\.toggleAttribute\('data-phone-shell', phoneShell\)/)
  assert.match(app, /\{!phoneShell && \(\s*<ShellSidebar\b/)
  assert.match(app, /\{phoneShell && \(\s*<Suspense fallback=\{null\}>\s*<PhoneDrawer\b/)
  assert.match(app, /const phoneTopBar = phoneShell && page !== 'session'/)
  assert.match(app, /\{phoneTopBar && \(\s*<Suspense[\s\S]{0,160}<PhoneTopBar\b/)
  assert.doesNotMatch(app, /ShellTabBar|tabBarFallback/)
  // 平板仍按移动端处理原生返回（BackNavigation）与软键盘，只是布局换成桌面。
  assert.match(app, /<BackNavigation onRouteBack=\{mobileLayout \? routeBack : undefined\} \/>/)
})

test('phone-only surfaces read the shared phone-shell gate instead of a viewport check', async () => {
  // 手机全部代理、手机会话页（顶栏 ☰）、功能里的组件行、组件列表 → 详情、主页草稿
  // 与会话审批的底部确认表单：都只在 usePhoneShell() 为真时出现。
  const gated = {
    'src/features/board/BoardPage.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/SessionShell.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/MorePage.tsx': /const phone = usePhoneShell\(\)/,
    'src/app/SessionHome.tsx': /const phone = usePhoneShell\(\)/,
    'src/features/custom-ui/CustomUiPage.tsx': /const phone = usePhoneShell\(\) && !embedded/,
    'src/features/chat/ToolApproval.tsx': /const phone = usePhoneShell\(\)/,
  }
  for (const [file, pattern] of Object.entries(gated))
    assert.match(await readFile(file, 'utf8'), pattern, file)
  const board = await readFile('src/features/board/BoardPage.tsx', 'utf8')
  // 手机全部代理与桌面是同一页的两种形态（规格 v2 第 7.3 节）：切换外壳时用 key 整页换掉，
  // 不沿用另一种形态的焦点与滚动状态。
  assert.match(
    board,
    /return <OverviewPage key=\{phone \? 'phone' : 'desktop'\} \{\.\.\.props\} phone=\{phone\} \/>/,
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

test('every phone approval entry keeps critical approvals out of one-step approval', async () => {
  const [board, row, toolApproval, confirm, menuButton, topBar, drawer, header, sessions] =
    await Promise.all(
      [
        'src/features/board/BoardPage.tsx',
        'src/features/board/OverviewRow.tsx',
        'src/features/chat/ToolApproval.tsx',
        'src/features/board/ApprovalConfirm.tsx',
        'src/app/PhoneMenuButton.tsx',
        'src/app/PhoneTopBar.tsx',
        'src/app/PhoneDrawer.tsx',
        'src/features/chat/SessionPhoneHeader.tsx',
        'src/features/board/SidebarSessions.tsx',
      ].map((file) => readFile(file, 'utf8')),
    )
  // 手机全部代理（规格 7.3）：卡片与桌面行同一个 OverviewRow，“允许”只在 sidebarQuickAction 判为 decide 的
  // 普通批准上出现，高风险只有“打开处理”（进会话按住确认），控制器之外不批准。
  assert.match(row, /if \(quick\.kind === 'decide' && approval\) \{/)
  assert.match(row, /onClick=\{\(\) => handlers\.onApprove\(item, approval\)\}/)
  assert.doesNotMatch(board + row, /approveBoardApproval|confirmCritical|\.approve\(/)
  // 手机会话页的审批面板：critical 的“允许…”只打开确认层（底部表单形态）。
  assert.match(
    toolApproval,
    /onClick=\{\(\) => \(critical \? setConfirming\(true\) : resolve\(true\)\)\}/,
  )
  assert.match(toolApproval, /<ApprovalConfirm[\s\S]{0,300}phone=\{phone\}/)
  // 确认层：手机是底部表单；按住 600ms 才确认，点击不能绕过，详情未到时不能按住。
  assert.match(confirm, /export const APPROVAL_HOLD_MS = 600/)
  assert.match(
    confirm,
    /if \(phone\)\s*return \(\s*<Sheet open=\{open\} onOpenChange=\{onOpenChange\}>/,
  )
  assert.match(confirm, /onClick=\{\(event\) => event\.preventDefault\(\)\}/)
  assert.match(confirm, /disabled=\{pending \|\| !approval\}/)
  // 抽屉里的一步处理只给普通批准（sidebarQuickAction 判为 decide 的行），高风险与详情未到的行只能打开；
  // 批准走与侧栏快捷处理同一个 decide（confirmCritical: false，Runtime 认为需要确认时改为打开会话）。
  assert.match(
    sessions,
    /\{drawer && row\.group === 'attention' && row\.quickAction\.kind === 'decide' && \(/,
  )
  assert.match(
    sessions,
    /actionsRef\.current\.approve\(row\.id, approvalId, \{ confirmCritical: false \}\)/,
  )
  // ☰、手机顶栏、抽屉外壳与会话顶栏只显示计数或跳转，从不批准。
  for (const [file, source] of [
    ['PhoneMenuButton', menuButton],
    ['PhoneTopBar', topBar],
    ['PhoneDrawer', drawer],
    ['SessionPhoneHeader', header],
  ])
    assert.doesNotMatch(
      source,
      /\.approve\(|approveBoardApproval|confirmCritical|resolve\(true\)/,
      file,
    )
})
