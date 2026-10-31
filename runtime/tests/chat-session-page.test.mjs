// 聊天页（只由会话路由挂载）：会话打开请求的格式、单会话视图的接线，以及手机 WebView 的客户端识别。
// A4 随手机外壳删除了 Dockview 分屏的布局持久化、面板 id 与手机会话标签（dock-layout / use-chat-dock /
// MobileSessionPanel），对应的纯逻辑测试一起删除；Runtime 的布局存储接口不变，前端只是不再读写。
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  createSessionOpenRequest,
  parseSessionOpenRequest,
} from '../../src/features/chat/session-open-request.ts'

test('session open requests keep parsing the old split dispositions', () => {
  // 旧版本留下的请求（拆分到左 / 上等）照常解析并打开会话，不因分屏退役被丢弃。
  assert.deepEqual(createSessionOpenRequest('alpha', 'left'), {
    sessionId: 'alpha',
    disposition: 'left',
  })
  assert.deepEqual(createSessionOpenRequest('alpha', 'above'), {
    sessionId: 'alpha',
    disposition: 'above',
  })
  assert.deepEqual(createSessionOpenRequest('alpha', 'open', 'entry-1'), {
    sessionId: 'alpha',
    disposition: 'open',
    targetEntryId: 'entry-1',
  })
  assert.deepEqual(parseSessionOpenRequest('{"sessionId":"beta","disposition":"right"}'), {
    sessionId: 'beta',
    disposition: 'right',
  })
  assert.deepEqual(
    parseSessionOpenRequest('{"sessionId":"beta","disposition":"below","targetEntryId":"entry-2"}'),
    {
      sessionId: 'beta',
      disposition: 'below',
      targetEntryId: 'entry-2',
    },
  )
  assert.equal(createSessionOpenRequest('alpha', 'top'), null)
  assert.equal(parseSessionOpenRequest('{"sessionId":"alpha","disposition":"bottom"}'), null)
})

// 会话页（/board/:sessionId）是唯一挂载聊天页的路由，只挂 URL 指定的一个会话；桌面与手机都是
// “页头 + 对话 + 检查器”的会话工作区，分屏、多标签与手机会话标签都不存在（A4 删除了 Dockview 依赖）。
test('the session page mounts one session with the inspector and no dock, splits or mobile tabs', async () => {
  const [
    dock,
    focus,
    selection,
    history,
    page,
    catalog,
    app,
    header,
    shell,
    routes,
    workspace,
    pkg,
  ] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/use-session-selection.ts', 'utf8'),
    readFile('src/features/chat/ChatHistoryPage.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/PageHeader.tsx', 'utf8'),
    readFile('src/app/SessionShell.tsx', 'utf8'),
    readFile('src/app/route-elements.tsx', 'utf8'),
    readFile('src/features/chat/SessionWorkspace.tsx', 'utf8'),
    readFile('package.json', 'utf8'),
  ])
  // 聊天页只由会话路由经 SessionShell 挂载，并且总带着路由会话 id：聊天页永远处于路由模式。
  assert.equal(routes.match(/import\('@\/features\/chat\/ChatPage'\)/g)?.length, 1)
  assert.match(
    routes.slice(routes.indexOf('export async function sessionRoute')),
    /^export async function sessionRoute\(\) \{[\s\S]*?import\('@\/features\/chat\/ChatPage'\)/,
  )
  assert.match(shell, /<ChatPage[\s\S]*?routeSessionId=\{sessionId\}/)
  assert.match(page, /const routeMode = routeSessionId !== undefined/)
  assert.match(page, /openRoutedSession: routeMode \? onRouteSessionChange : undefined/)
  assert.doesNotMatch(page, /ChatDockView|MobileSessionPanel|useChatDock|splitAvailable/)
  assert.match(
    page,
    /\) : routeTarget \? \(\s*<SingleSessionPanel\s+sessionId=\{routeTarget\}\s+onCreateSession=\{createSession\}\s*\/>/,
  )
  const singlePanel = dock.slice(dock.indexOf('export function SingleSessionPanel'))
  assert.match(singlePanel, /key=\{activeId\}/)
  assert.doesNotMatch(
    dock,
    /SessionDockPanel|MobileSessionPanel|dockview|canSplitPanel|canClosePanel/,
  )
  assert.doesNotMatch(focus, /onSplit(?:Left|Right|Top|Bottom)|onClosePanel|<SessionActionsMenu/)
  assert.doesNotMatch(pkg, /dockview/)
  // 打开会话：路由模式交回路由切换，否则只切换活动会话；带定位的请求强制重读。
  assert.match(
    selection,
    /if \(sessionId && openRoutedSessionRef\.current\) openRoutedSessionRef\.current\(sessionId\)\s*else setActiveId\(sessionId\)/,
  )
  assert.match(
    selection,
    /if \(detail\?\.targetEntryId\)\s*void loadSessionMessages\(sessionId, \{ force: true/,
  )
  // 删除会话时清掉它的上下文页与未发送草稿。
  assert.match(
    selection,
    /useContextPagesStore\.getState\(\)\.forget\(id\)\s*clearComposerDraft\(id\)/,
  )
  // 会话页布局：面板渲染会话工作区（页头 + 对话 + 检查器），其余入口才是旧的任务工作区。
  assert.match(dock, /\{pageChrome \? \(\s*\/\/[^\n]*\n\s*<SessionWorkspace/)
  assert.match(dock, /<TaskWorkspace/)
  assert.match(workspace, /const SessionInspector = lazy\(/)
  assert.match(workspace, /<SessionInspector\s+panelId=\{panelId\}\s+sessionId=\{sessionId\}/)
  // 改名与换工作区：旧顶栏与会话页头都直接可点。
  assert.match(focus, /onClick=\{onRename\}/)
  assert.match(focus, /onClick=\{onWorkspace\}/)
  assert.match(focus, /onRename=\{onRename\}\s+onWorkspace=\{onWorkspace\}/)
  // 历史页打开会话只发选中请求，不提供分屏。
  assert.match(history, /<DropdownMenu>/)
  assert.match(history, /requestSessionSelection\(id\)/)
  assert.doesNotMatch(history, /splitTo(?:Left|Right|Top|Bottom)/)
  // 手机 App 新建会话：本机模式不继承桌面会话路径，已配对远程模式才继承。
  assert.match(page, /waitForMobileRuntimeReady/)
  assert.match(page, /mobile_state/)
  assert.match(page, /shouldInheritRecentSessionCwd\(mobileApp, mobileState\)/)
  assert.match(page, /inheritRecentCwd/)
  assert.match(catalog, /inheritRecentCwd = true/)
  assert.match(catalog, /sessionCwdForCreate\(cwd, sessionsRef\.current, inheritRecentCwd\)/)
  assert.match(page, /preserveExistingOnEmpty: true/)
  assert.match(app, /mobileApp=\{mobileApp\}/)
  assert.match(header, /page === 'chat'[\s\S]*mobileApp[\s\S]*newChat/)
  assert.match(header, /page === 'chat' && mobileApp && 'hidden'/)
  assert.doesNotMatch(focus, /WorkspaceTrustNotice/)
})

test('移动 WebView 在 API 握手前也稳定识别为移动客户端', async () => {
  const [mobileShell, clientStore] = await Promise.all([
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
    readFile('src/stores/client-store.ts', 'utf8'),
  ])
  assert.match(mobileShell, /MOBILE_CLIENT_INITIALIZATION_SCRIPT/)
  assert.match(mobileShell, /__PISPER_MOBILE_APP__/)
  assert.match(mobileShell, /\.initialization_script\(MOBILE_CLIENT_INITIALIZATION_SCRIPT\)/)
  assert.match(clientStore, /window\.__PISPER_MOBILE_APP__ === true/)
  assert.match(clientStore, /client: isNativeMobileApp\(\) \? 'mobile-app' : 'web'/)
  assert.match(clientStore, /nativeMobileApp \? 'mobile-app' : 'web'/)
})

test('移动端前台恢复会先校准本机 Runtime，再保留瞬时空目录', async () => {
  const [page, catalog, shell] = await Promise.all([
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
  ])
  assert.match(page, /await waitForMobileRuntimeReady\(\)/)
  assert.match(page, /refreshSessions\(undefined, \{ preserveExistingOnEmpty: true \}\)/)
  assert.match(catalog, /preserveExistingOnEmpty = false/)
  assert.match(catalog, /!data\.sessions\.length && sessionsRef\.current\.length/)
  assert.match(shell, /async fn mobile_resume_local_runtime[\s\S]*resume_remote_network\(\)\.await/)
  assert.match(shell, /ensure_local_runtime_ready\(state\.on_device\.clone\(\)/)
  assert.match(
    shell,
    /RunEvent::Resumed[\s\S]*resume_remote_network\(\)\.await[\s\S]*ensure_local_runtime_ready/,
  )
})

// 回滚保护：Runtime 里存着的旧分屏布局（/api/settings/chat-dock-layout）前端既不读也不写，
// 回滚到仍有分屏的旧版本时布局原样可用；接口本身留在 Runtime（只加不改）。
test('the frontend never reads or overwrites the stored split layout', async () => {
  const { readdir } = await import('node:fs/promises')
  const { join } = await import('node:path')
  const offenders = []
  const scan = async (directory) => {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name)
      if (entry.isDirectory()) await scan(path)
      else if (/\.(?:ts|tsx)$/.test(entry.name)) {
        if ((await readFile(path, 'utf8')).includes('/api/settings/chat-dock-layout'))
          offenders.push(path)
      }
    }
  }
  await scan('src')
  assert.deepEqual(offenders, [])
})
