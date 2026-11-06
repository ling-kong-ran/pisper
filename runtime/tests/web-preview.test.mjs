import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  normalizeWebPreviewInput,
  shouldOpenWebPreview,
  webPreviewTabLabel,
} from '../../src/lib/web-preview.ts'
import { openWebPreviewInSessionContext } from '../../src/features/chat/session-web-preview.ts'
import { useContextPagesStore } from '../../src/features/chat/session-context-pages-store.ts'
import {
  activeBrowserPage,
  createContextPages,
  MAX_CONTEXT_PAGES,
  updateContextPages,
} from '../../src/features/chat/session-context-pages.ts'
import { previewSessionTarget } from '../../src/app/web-preview-target.ts'
import {
  isOccluded,
  occlusionProbePoints,
  overlapsAny,
  placementChanged,
  placementFromRect,
} from '../../src/features/chat/native-browser-geometry.ts'
import {
  EXIT_PRESENCE_FALLBACK_MS,
  exitPresenceMounted,
  finishExitPresence,
  nextExitPresence,
} from '../../src/hooks/useExitPresence.ts'

test('external http links open in the in-app preview while internal and special links keep native behavior', () => {
  const baseUrl = 'http://127.0.0.1:5173/chat'
  assert.equal(
    shouldOpenWebPreview({ href: 'https://example.com/docs', baseUrl }),
    'https://example.com/docs',
  )
  assert.equal(shouldOpenWebPreview({ href: '/config', baseUrl }), null)
  assert.equal(shouldOpenWebPreview({ href: 'mailto:hello@example.com', baseUrl }), null)
  assert.equal(
    shouldOpenWebPreview({ href: 'https://example.com/file', baseUrl, download: true }),
    null,
  )
  assert.equal(shouldOpenWebPreview({ href: 'https://example.com', baseUrl, ctrlKey: true }), null)
  assert.equal(
    shouldOpenWebPreview({
      href: 'https://example.com',
      baseUrl,
      behavior: 'external',
    }),
    null,
  )
})

test('URL bar input and dock panel metadata normalize preview destinations', () => {
  assert.equal(
    normalizeWebPreviewInput('example.com/guide', 'http://127.0.0.1:5173/chat'),
    'https://example.com/guide',
  )
  assert.equal(
    normalizeWebPreviewInput('https://example.com', 'http://127.0.0.1:5173/chat'),
    'https://example.com/',
  )
  assert.equal(normalizeWebPreviewInput('javascript:alert(1)', 'http://127.0.0.1:5173/chat'), null)
})

test('URL bar reads host:port as an address and opens local servers over http', () => {
  const base = 'http://127.0.0.1:5173/chat'
  for (const [input, expected] of [
    ['localhost:5173', 'http://localhost:5173/'],
    ['localhost:5173/docs?a=1#b', 'http://localhost:5173/docs?a=1#b'],
    ['localhost', 'http://localhost/'],
    ['localhost:443', 'http://localhost:443/'],
    ['app.localhost:3000', 'http://app.localhost:3000/'],
    ['printer.local', 'http://printer.local/'],
    ['127.0.0.1:3000', 'http://127.0.0.1:3000/'],
    ['0.0.0.0:8000', 'http://0.0.0.0:8000/'],
    ['[::1]:3000', 'http://[::1]:3000/'],
    ['10.0.0.8:3000', 'http://10.0.0.8:3000/'],
    ['172.20.1.2:3000', 'http://172.20.1.2:3000/'],
    ['192.168.1.20:8080', 'http://192.168.1.20:8080/'],
    // 公网地址和域名仍然默认 https；用户写明的协议不改。
    ['example.com:8080', 'https://example.com:8080/'],
    ['example.com:8080/a', 'https://example.com:8080/a'],
    ['172.32.1.2:3000', 'https://172.32.1.2:3000/'],
    ['8.8.8.8', 'https://8.8.8.8/'],
    ['https://localhost:5173', 'https://localhost:5173/'],
    ['http://example.com:8080', 'http://example.com:8080/'],
  ]) {
    assert.equal(normalizeWebPreviewInput(input, base), expected, input)
  }
  // 端口判断不能给脚本协议开口子：结果要么被拒绝，要么只是 http(s) 地址。
  for (const input of [
    'javascript:alert(1)',
    'javascript:1,alert(1)',
    'data:text/html,x',
    'file:///etc/passwd',
    'localhost:99999',
  ]) {
    assert.equal(normalizeWebPreviewInput(input, base), null, input)
  }
  assert.equal(normalizeWebPreviewInput('javascript:12', base), 'https://javascript:12/')
})

test('application routes external links into the session inspector web preview', async () => {
  const [app, provider, chatPage, workspace, inspector, browserPane, component] = await Promise.all(
    [
      readFile('src/App.tsx', 'utf8'),
      readFile('src/app/WebPreviewProvider.tsx', 'utf8'),
      readFile('src/features/chat/ChatPage.tsx', 'utf8'),
      readFile('src/features/chat/SessionWorkspace.tsx', 'utf8'),
      readFile('src/features/chat/SessionInspector.tsx', 'utf8'),
      readFile('src/features/chat/SessionBrowserPane.tsx', 'utf8'),
      readFile('src/components/ai-elements/web-preview.tsx', 'utf8'),
    ],
  )

  assert.match(app, /<WebPreviewProvider \/>/)
  assert.match(provider, /requestWebPreview\(url\)/)
  // 不在会话页时带着请求去最近的有效活动会话，不再回旧的 /chat 多标签页；
  // 没有有效会话时在外部浏览器打开，绝不为了显示网页去 /board/new 新建空会话。
  assert.match(
    provider,
    /const sessionId = previewSessionTarget\(\s*pageStateStorage\.getItem\(STORAGE_KEYS\.activeSession\),\s*queryClient\.getQueryData<CachedSessions>\(SESSIONS_QUERY_KEY\),?\s*\)/,
  )
  assert.match(
    provider,
    /if \(!sessionId\) \{\s*window\.open\(url, '_blank', 'noopener,noreferrer'\)\s*return\s*\}\s*requestWebPreview\(url\)\s*navigate\(sessionPath\(sessionId\)\)/,
  )
  assert.doesNotMatch(provider, /sessionPath\([^)]*\|\| undefined\)|sessionPath\(\)/)
  assert.doesNotMatch(provider, /pagePath\('chat'\)/)
  // 会话页（单会话布局，没有 Dockview）：路由会话确认存在后才补开遗留请求，
  // 之后的预览事件落到当前会话检查器；“找不到这个会话”时请求不被吃掉。
  assert.match(
    chatPage,
    /if \(!routeTarget \|\| !routeKnown\) return\s+const pending = consumeWebPreviewRequest\(\)/,
  )
  assert.match(chatPage, /const pending = consumeWebPreviewRequest\(\)/)
  assert.match(
    chatPage,
    /if \(pending\) openWebPreviewInSessionContext\(routeTarget, pending\.url\)/,
  )
  assert.match(chatPage, /if \(url\) openWebPreviewInSessionContext\(routeTarget, url\)/)
  assert.match(chatPage, /addEventListener\(WEB_PREVIEW_OPEN_EVENT, openPreview\)/)
  // 第 3 期定案：桌面多标签 Dock 的右侧预览面板随 Dockview 一起退役（不再挂载），
  // 预览落到会话页检查器的“预览”标签。请求写进会话上下文页 store，检查器订阅它：
  // 浏览器页被激活或换了地址就切到“预览”标签并打开检查器。
  assert.match(workspace, /case 'browser':\s*return \{ tab: 'preview' \}/)
  assert.match(workspace, /useContextPagesStore\.subscribe\(\(state, previous\) => \{/)
  assert.match(workspace, /\(page\.kind === 'browser' && page\.url !== previousPage\?\.url\)/)
  assert.match(workspace, /if \(changed\) reveal\(page\.kind\)/)
  assert.match(inspector, /if \(tab === 'preview'\) return 'browser'/)
  assert.match(inspector, /<BrowserTabs key=\{sessionId\}/)
  assert.match(
    await readFile('src/features/chat/SessionBrowserTabs.tsx', 'utf8'),
    /<BrowserPane[\s\S]*?update\(sessionId, 'browser', \{ type: 'browser', id: page\.id, \.\.\.patch \}\)/,
  )
  // 检查器收起时整个面板不渲染：预览 iframe 只在“预览”标签可见时存在（取代 Dock 面板的可见性判断）。
  // 手机底部表单关闭后要播退场动画：面板只在退场期间多挂载，关闭且退场结束（动画结束、兜底超时或减少动效）
  // 即不渲染；sheetLeaving 只在手机表单时成立，覆盖层与并排检查器仍是关闭即卸载。退场状态机见下一条测试。
  assert.match(workspace, /const sheetPresence = useExitPresence\(sheetMode && open\)/)
  assert.match(workspace, /const sheetLeaving = sheetMode && sheetPresence\.leaving/)
  assert.match(workspace, /const inspector = \(open \|\| sheetLeaving\) && \(/)
  assert.match(
    workspace,
    /onAnimationEnd=\{\(event\) => \{\s*if \(event\.target === event\.currentTarget && !open\) sheetPresence\.finish\(\)/,
  )
  assert.match(workspace, /\{!overlayMode && open && \(/)
  assert.match(inspector, /\} else if \(tab === 'preview'\) \{/)
  assert.match(browserPane, /window\.open\(url, '_blank', 'noopener,noreferrer'\)/)
  assert.match(browserPane, /normalizeWebPreviewInput\(draft, window\.location\.href\)/)
  assert.match(browserPane, /<WebPreviewBody\s+src=\{url\}/)
  assert.match(
    component,
    /sandbox="allow-scripts allow-same-origin allow-forms allow-popups allow-presentation"/,
  )
  assert.match(component, /WebPreviewContext\.Provider/)
})

test('the phone inspector sheet stays mounted only for its exit transition', async () => {
  const presence = await readFile('src/hooks/useExitPresence.ts', 'utf8')
  const closed = { open: false, leaving: false }
  const shown = nextExitPresence(closed, true)
  assert.deepEqual(shown, { open: true, leaving: false })
  assert.equal(exitPresenceMounted(shown), true)
  // 关闭：同一次渲染进入退场，仍然挂载（播放退场动画）。
  const leaving = nextExitPresence(shown, false)
  assert.deepEqual(leaving, { open: false, leaving: true })
  assert.equal(exitPresenceMounted(leaving), true)
  // 关闭且退场结束 → 不渲染（预览 iframe 随之卸载）。
  const gone = finishExitPresence(leaving)
  assert.deepEqual(gone, closed)
  assert.equal(exitPresenceMounted(gone), false)
  assert.equal(exitPresenceMounted(closed), false)
  // 退场途中重新打开：取消退场；迟到的结束事件或兜底超时不会把打开的表单卸掉。
  const reopened = nextExitPresence(leaving, true)
  assert.deepEqual(reopened, { open: true, leaving: false })
  assert.equal(finishExitPresence(reopened), reopened)
  assert.equal(nextExitPresence(reopened, true), reopened)
  // 兜底超时有上限：漏掉 animationend 时表单也不会一直挂着；减少动效时不等动画，绘制前就卸载。
  assert.ok(EXIT_PRESENCE_FALLBACK_MS > 180 && EXIT_PRESENCE_FALLBACK_MS <= 1000)
  assert.match(
    presence,
    /if \(motionReduced\(\)\) \{\s*finish\(\)\s*return\s*\}\s*const timer = window\.setTimeout\(finish, fallbackMs\)\s*return \(\) => window\.clearTimeout\(timer\)/,
  )
})

test('link previews only target a live, unarchived session from the catalog cache', () => {
  const data = {
    sessions: [{ id: 'live' }, { id: 'archived', archived: true }],
  }
  assert.equal(previewSessionTarget('live', data), 'live')
  // 删除或归档的会话、从没打开过会话、目录还没加载：都没有目标，改为外部打开。
  assert.equal(previewSessionTarget('archived', data), '')
  assert.equal(previewSessionTarget('deleted', data), '')
  assert.equal(previewSessionTarget(null, data), '')
  assert.equal(previewSessionTarget('live', undefined), '')
})

test('session web preview opens links in tabs without losing what is already open', () => {
  const store = () => useContextPagesStore.getState()
  const browserPages = (sessionId) =>
    (store().sessions[sessionId]?.pages || []).filter((page) => page.kind === 'browser')
  assert.equal(openWebPreviewInSessionContext('', 'https://example.com/'), false)
  assert.equal(openWebPreviewInSessionContext('preview-session', ''), false)
  try {
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/a'), true)
    const [first] = browserPages('preview-session')
    assert.equal(first.url, 'https://example.com/a')
    assert.equal(store().sessions['preview-session'].activeId, first.id)
    // 新地址开新页签，原来的网页还在。
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/b'), true)
    const pages = browserPages('preview-session')
    assert.deepEqual(
      pages.map((page) => page.url),
      ['https://example.com/a', 'https://example.com/b'],
    )
    assert.equal(store().sessions['preview-session'].activeBrowserId, pages[1].id)
    // 已经开着的地址切过去，不再多开。
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/a'), true)
    assert.equal(browserPages('preview-session').length, 2)
    assert.equal(store().sessions['preview-session'].activeBrowserId, first.id)
    // 当前是空白页签时直接用它。
    store().update('preview-session', 'browser', { type: 'add', kind: 'browser' })
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/c'), true)
    assert.equal(browserPages('preview-session').length, 3)
    assert.equal(browserPages('preview-session')[2].url, 'https://example.com/c')
    // 预览只落在发起的会话里，不影响其他会话的检查器。
    assert.equal(browserPages('other-session').length, 0)
    // 开满后在当前页签打开，链接不能白点。
    for (let index = 0; index < MAX_CONTEXT_PAGES; index++)
      store().update('preview-session', 'browser', { type: 'add', kind: 'files' })
    while (store().sessions['preview-session'].pages.length < MAX_CONTEXT_PAGES)
      store().update('preview-session', 'browser', { type: 'add', kind: 'browser' })
    const before = browserPages('preview-session').length
    assert.equal(openWebPreviewInSessionContext('preview-session', 'https://example.com/d'), true)
    assert.equal(browserPages('preview-session').length, before)
    assert.ok(browserPages('preview-session').some((page) => page.url === 'https://example.com/d'))
  } finally {
    store().forget('preview-session')
  }
})

test('browser tabs have their own selection and closing one stays on a browser tab', () => {
  let state = createContextPages('browser')
  state = updateContextPages(state, { type: 'add', kind: 'files' })
  state = updateContextPages(state, { type: 'add', kind: 'browser' })
  state = updateContextPages(state, { type: 'add', kind: 'browser' })
  assert.equal(activeBrowserPage(state).id, 'page-4')
  // 切页签不碰“只显示内容”和当前页。
  const hidden = updateContextPages(state, { type: 'workspace', contentOpen: false })
  const switched = updateContextPages(hidden, { type: 'browser-tab', id: 'page-3' })
  assert.equal(activeBrowserPage(switched).id, 'page-3')
  assert.equal(switched.contentOpen, false)
  assert.equal(switched.activeId, hidden.activeId)
  // 别的种类不能被选成浏览器页签。
  assert.equal(updateContextPages(state, { type: 'browser-tab', id: 'page-2' }), state)
  // 关掉最右边的落到左边的浏览器页签，不落到中间的“文件”上，否则检查器会跳走。
  state = updateContextPages(state, { type: 'close', id: 'page-4' })
  assert.equal(state.activeId, 'page-3')
  assert.equal(activeBrowserPage(state).id, 'page-3')
  state = updateContextPages(state, { type: 'close', id: 'page-3' })
  assert.equal(state.activeId, 'page-1')
  assert.equal(activeBrowserPage(state).id, 'page-1')
  // 换地址时旧标题作废；标题和地址同时给时以给的为准。
  state = updateContextPages(state, { type: 'browser', id: 'page-1', title: 'Old' })
  assert.equal(activeBrowserPage(state).title, 'Old')
  state = updateContextPages(state, { type: 'browser', id: 'page-1', draft: 'typing' })
  assert.equal(activeBrowserPage(state).title, 'Old')
  state = updateContextPages(state, { type: 'browser', id: 'page-1', url: 'https://b.example/' })
  assert.equal(activeBrowserPage(state).title, '')
})

test('browser tab labels fall back to the host or the previewed file', () => {
  assert.equal(webPreviewTabLabel('https://github.com/login'), 'github.com')
  assert.equal(webPreviewTabLabel('http://localhost:5173/'), 'localhost:5173')
  assert.equal(
    webPreviewTabLabel('http://127.0.0.1:1/_pisper/html-preview/token/%E6%8A%A5%E5%91%8A.html'),
    '报告.html',
  )
  assert.equal(webPreviewTabLabel('not a url'), 'not a url')
})

test('a hidden browser tab keeps its size so the page does not lay out at zero width', () => {
  const rect = { left: 10, top: 20, width: 300, height: 200 }
  const shown = placementFromRect(null, rect, false)
  assert.deepEqual(shown, { bounds: { x: 10, y: 20, width: 300, height: 200 }, visible: true })
  const hidden = placementFromRect(shown, { left: 0, top: 0, width: 0, height: 0 }, false)
  assert.deepEqual(hidden, { bounds: shown.bounds, visible: false })
  assert.deepEqual(placementFromRect(shown, rect, true), { bounds: shown.bounds, visible: false })
})

test('the native browser follows its slot and gets out of the way of overlays', () => {
  const bounds = { x: 10, y: 20, width: 300, height: 200 }
  const placed = { bounds, visible: true }
  assert.equal(placementChanged(null, placed), true)
  assert.equal(placementChanged(placed, { bounds, visible: false }), true)
  assert.equal(placementChanged(placed, { bounds: { ...bounds, x: 10.2 }, visible: true }), false)
  assert.equal(placementChanged(placed, { bounds: { ...bounds, width: 301 }, visible: true }), true)

  const rect = { left: 100, top: 100, width: 100, height: 100 }
  const viewport = { width: 1000, height: 800 }
  const points = occlusionProbePoints(rect, viewport)
  assert.equal(points.length, 9)
  const slot = {}
  assert.equal(
    isOccluded(slot, points, () => slot),
    false,
  )
  // 弹层盖住一部分，或模态弹层让 body 不可点（命中根元素），都要让路。
  assert.equal(
    isOccluded(slot, points, (x, y) => (x > 150 && y > 150 ? {} : slot)),
    true,
  )
  assert.equal(
    isOccluded(slot, points, () => ({ tagName: 'HTML' })),
    true,
  )
  // 碰不到探测点的小菜单也要让路；只是贴着边不算。
  assert.equal(overlapsAny(rect, [{ left: 195, top: 95, width: 10, height: 10 }]), true)
  assert.equal(overlapsAny(rect, [{ left: 200, top: 100, width: 10, height: 10 }]), false)
  assert.equal(overlapsAny(rect, []), false)
  // 伸出窗口的部分不探测，否则窗口变窄时网页会一直被当成遮住。
  const clipped = occlusionProbePoints({ ...rect, left: 950 }, viewport)
  assert.ok(clipped.length > 0 && clipped.every((point) => point.x < viewport.width))
  assert.deepEqual(occlusionProbePoints(rect, { width: 0, height: 0 }), [])
})

test('the native browser stays isolated from the app and leaves the main window untouched', async () => {
  const [browser, cargo, capability, permission, config, bridge, pane] = await Promise.all([
    readFile('src-tauri/src/desktop_shell/desktop_browser.rs', 'utf8'),
    readFile('src-tauri/Cargo.toml', 'utf8'),
    readFile('src-tauri/capabilities/desktop-browser.json', 'utf8'),
    readFile('src-tauri/permissions/browser.toml', 'utf8'),
    readFile('src-tauri/tauri.conf.json', 'utf8'),
    readFile('src-tauri/src/desktop_shell/desktop-bridge.js', 'utf8'),
    readFile('src/features/chat/SessionBrowserPane.tsx', 'utf8'),
  ])
  // 网页拿不到 IPC，也不走 Tauri 的 WebView 管理；主窗口不因此改成子 WebView。
  assert.doesNotMatch(browser, /with_ipc_handler|with_initialization_script/)
  assert.match(browser, /build_as_child\(window\)/)
  assert.doesNotMatch(cargo, /"unstable"/)
  // 数据隔离：macOS 14 之前没有独立存储时要用无痕，不能退回和主窗口共用的默认存储。
  assert.match(browser, /with_data_store_identifier\(PROFILE_STORE\)/)
  assert.match(browser, /with_incognito\(true\)/)
  assert.match(browser, /WebContext::new\(Some\(profile\)\)/)
  // 只有主窗口能操作。
  assert.deepEqual(JSON.parse(capability).windows, ['main'])
  assert.match(permission, /identifier = "allow-pisper-desktop-browser"/)
  assert.ok(JSON.parse(config).app.security.capabilities.includes('desktop-browser'))
  for (const command of ['open', 'navigate', 'action', 'sync', 'close']) {
    assert.match(permission, new RegExp(`"desktop_browser_${command}"`))
    assert.match(bridge, new RegExp(`'desktop_browser_${command}'`))
    assert.match(
      browser,
      new RegExp(`pub async fn desktop_browser_${command}\\([\\s\\S]*?ensure_main\\(&window\\)\\?`),
    )
  }
  // 新窗口开成新页签，不让系统另开一个窗口。
  assert.match(browser, /BrowserEvent::NewWindow \{ url: target \}/)
  assert.match(browser, /NewWindowResponse::Deny/)
  // 不支持时明确退回 iframe，不留空白。
  assert.match(browser, /status: "unsupported"/)
  assert.match(pane, /native\.active \?[\s\S]*: url \?[\s\S]*<WebPreview/)
})
