import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import { DEV_WATCH_IGNORES, vendorChunkForModule } from '../../vite.config.ts'

test('vendor chunk classification is stable and preserves Shiki dynamic modules', () => {
  const modulePath = (name, file = 'index.js') => `C:/repo/node_modules/${name}/${file}`

  assert.equal(vendorChunkForModule(modulePath('react-dom', 'client.js')), 'vendor-react')
  assert.equal(vendorChunkForModule(modulePath('dockview-core')), 'vendor-dockview')
  assert.equal(vendorChunkForModule(modulePath('@xyflow/react')), 'vendor-xyflow')
  assert.equal(vendorChunkForModule(modulePath('d3-selection')), 'vendor-xyflow')
  assert.equal(vendorChunkForModule(modulePath('motion', 'react.js')), 'vendor-motion')
  assert.equal(vendorChunkForModule(modulePath('streamdown')), 'vendor-markdown')
  assert.equal(vendorChunkForModule(modulePath('@streamdown/code')), 'vendor-markdown-plugins')
  assert.equal(
    vendorChunkForModule(modulePath('shiki', 'dist/bundle-web.mjs')),
    'vendor-shiki-runtime',
  )
  assert.equal(vendorChunkForModule(modulePath('@shikijs/core')), 'vendor-shiki-runtime')
  assert.equal(vendorChunkForModule(modulePath('zustand')), 'vendor-state')
  assert.equal(vendorChunkForModule(modulePath('tailwind-merge')), 'vendor-ui')

  assert.equal(vendorChunkForModule(modulePath('@shikijs/langs', 'dist/typescript.mjs')), undefined)
  assert.equal(
    vendorChunkForModule(modulePath('@shikijs/themes', 'dist/github-dark.mjs')),
    undefined,
  )
  assert.equal(vendorChunkForModule(modulePath('shiki', 'dist/wasm.mjs')), undefined)
  assert.equal(
    vendorChunkForModule(modulePath('@shikijs/engine-oniguruma', 'dist/wasm-inlined.mjs')),
    undefined,
  )
})

test('development watcher excludes generated dependency and package trees', () => {
  for (const pattern of [
    '**/node_modules/**',
    '**/release/**',
    '**/src-tauri/target/**',
    '**/src-tui/target/**',
    '**/src-tauri/binaries/**',
  ]) {
    assert.ok(DEV_WATCH_IGNORES.includes(pattern), `missing watcher ignore: ${pattern}`)
  }
})

test('production build emits an audited manifest with explicit non-recursive chunk ownership', async () => {
  const [config, packageJson, buildScript] = await Promise.all([
    readFile('vite.config.ts', 'utf8'),
    readFile('package.json', 'utf8'),
    readFile('scripts/build-frontend.mjs', 'utf8'),
  ])
  const scripts = JSON.parse(packageJson).scripts

  assert.match(config, /manifest: true/)
  assert.match(config, /codeSplitting:/)
  assert.match(config, /includeDependenciesRecursively: false/)
  assert.match(config, /VENDOR_CHUNK_PRIORITIES\.map/)
  assert.match(
    scripts.build,
    /node scripts\/build-frontend\.mjs && node scripts\/check-bundle-budget\.mjs && node scripts\/check-dist-compat\.mjs/,
  )
  // iOS 最低 15.1：build.target 锁定 safari16（降级依赖产物里的 class static block，
  // 又不会像 safari15 那样把 vendor-shiki-runtime 拖进入口静态图）。
  assert.match(config, /target: 'safari16'/)
  assert.match(buildScript, /NODE_ENV: 'production'/)
  assert.match(buildScript, /'vite'/)
})

test('desktop terminal reattaches its cached renderer without taking ownership of the PTY', async () => {
  const [renderer, provider, view] = await Promise.all([
    readFile('src/features/terminal/terminal-renderer.ts', 'utf8'),
    readFile('src/features/terminal/TerminalProvider.tsx', 'utf8'),
    readFile('src/features/terminal/TaskTerminalView.tsx', 'utf8'),
  ])
  assert.match(renderer, /host\.append\(element\)/)
  assert.match(renderer, /observer\.observe\(host\)/)
  assert.match(provider, /renderers\.get\(key\)/)
  assert.match(view, /return \(\) => renderer\.detach\(\)/)
  assert.doesNotMatch(renderer, /terminalClose/)
})

test('desktop terminal resources retain original task scope while views select the current task', async () => {
  const [app, panel, registry, scope] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/terminal/TerminalPanel.tsx', 'utf8'),
    readFile('src/features/terminal/terminal-registry.ts', 'utf8'),
    readFile('src/features/terminal/terminal-session-scope.ts', 'utf8'),
  ])
  assert.match(app, /<TerminalProvider bridge=\{window\.pisperDesktop\}/)
  assert.match(app, /activeSessionId=\{activeSessionId\}/)
  assert.match(app, /resolveSessionCwd=\{resolveSessionCwd\}/)
  assert.match(panel, /record\.sessionId === activeSessionId \|\| record\.orphaned/)
  assert.match(registry, /sessionId: string/)
  assert.match(scope, /terminal\.sessionId === activeSessionId/)
  assert.doesNotMatch(scope, /sessionId: fallbackSessionId/)
  assert.doesNotMatch(panel, /terminalClose/)
})

test('opening the desktop terminal preserves a shrinkable chat layout above it', async () => {
  const terminal = await readFile('src/features/terminal/TerminalPanel.tsx', 'utf8')
  assert.match(terminal, /CHAT_RESERVED_HEIGHT = 420/)
  assert.match(terminal, /maximumTerminalHeight\(window\.innerHeight\)/)
  const app = await readFile('src/App.tsx', 'utf8')
  const chat = await readFile('src/features/chat/ChatPage.tsx', 'utf8')
  assert.match(app, /page === 'chat' \? 'page-chat flex overflow-hidden p-0'/)
  assert.match(chat, /chat-layout[^"\n]*min-h-0[^"\n]*flex-1/)
  assert.doesNotMatch(chat, /chat-layout[^"\n]*min-h-\[510px\]/)
})

test('terminal views may unmount while the application owns resources and theme adaptation', async () => {
  const [renderer, provider, styles] = await Promise.all([
    readFile('src/features/terminal/terminal-renderer.ts', 'utf8'),
    readFile('src/features/terminal/TerminalProvider.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])
  assert.match(provider, /registry\.dispose\(\)/)
  assert.match(provider, /window\.addEventListener\('pagehide', dispose\)/)
  // 终端底色跟随 v2 暖调中性灰（规格 6.2）：浅色与侧栏同为 #f7f7f5，深色与侧栏同为 #171716。
  assert.match(styles, /--terminal-bg: #f7f7f5;/)
  assert.match(styles, /:root\[data-theme='dark'\][\s\S]*?--terminal-bg: #171716;/)
  assert.match(renderer, /document\.documentElement\.dataset\.theme === 'dark'/)
  assert.match(renderer, /background: color\('--terminal-bg'/)
  assert.match(renderer, /themeObserver\.disconnect\(\)/)
  assert.match(
    styles,
    /\.terminal-xterm \.xterm \.xterm-viewport \{ background-color: var\(--terminal-bg\); \}/,
  )
})

test('workflow notifications separate system permission from external channel setup', async () => {
  const [inspector, editor, switchPrimitive] = await Promise.all([
    readFile('src/features/workflows/WorkflowNodeInspector.tsx', 'utf8'),
    readFile('src/features/workflows/useWorkflowEditor.ts', 'utf8'),
    readFile('src/components/ui/switch.tsx', 'utf8'),
  ])
  const notificationSwitch = inspector.match(
    /function NodeNotificationSettings[\s\S]*?<Switch[\s\S]*?onCheckedChange=\{\(\) => void onToggleNotification\(id\)\}/,
  )?.[0]
  const workflowSettings = inspector.match(
    /function WorkflowSettings[\s\S]*?function NodeNotificationSettings/,
  )?.[0]

  assert.ok(notificationSwitch)
  assert.ok(workflowSettings)
  assert.match(
    inspector,
    /id === 'browser'[\s\S]*?systemNotificationAvailable[\s\S]*?catalog\.notificationTargets\[id\]\?\.enabled/,
  )
  assert.match(
    inspector,
    /systemNotificationPermission === 'default' \|\| systemNotificationPermission === 'granted'/,
  )
  assert.match(notificationSwitch, /disabled=\{!targetEnabled\}/)
  assert.match(notificationSwitch, /node\.notificationTargets\.includes\(id\)/)
  assert.match(notificationSwitch, /node\.notification\.content/)
  assert.doesNotMatch(notificationSwitch, /draft\.notifications\.includes\(id\)/)
  assert.doesNotMatch(workflowSettings, /onToggleNotification/)
  assert.match(inspector, /noExternalNotificationChannelsEnabled/)
  assert.match(inspector, /systemNotificationPermissionRequired/)
  assert.match(inspector, /onOpenChannels/)
  assert.match(inspector, /onOpenSystemNotificationSettings/)
  assert.match(editor, /selectedNode\.kind !== 'notification'/)
  assert.match(editor, /updateNode\(\{ notificationTargets:/)
  assert.match(editor, /requestBrowserNotificationPermission/)
  assert.match(editor, /\/api\/settings\/notifications\/browser/)
  assert.match(editor, /notificationTargets\.filter/)
  assert.match(switchPrimitive, /inline-flex min-h-0! shrink-0/)
})

test('chat resource picker remains visible above dock splits with a readable primary action', async () => {
  const [picker, dialog, focusSession] = await Promise.all([
    readFile('src/features/chat/ChatResourcePicker.tsx', 'utf8'),
    readFile('src/components/ui/dialog.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
  ])

  assert.match(picker, /className="chat-resource-dialog[^"\n]*z-\[220\]/)
  assert.match(picker, /chat-resource-dialog[^"\n]*sm:max-w-\[820px\]/)
  assert.match(picker, /chat-resource-search[^"\n]*\[&_input\]:\[outline:0\]!/)
  assert.match(picker, /overlayClassName="z-\[220\]"/)
  assert.match(picker, /className="chat-resource-confirm[^"\n]*text-\[var\(--on-accent\)\]/)
  assert.match(picker, /apiJson<PluginsData>\(`\/api\/plugins\?sessionId=/)
  assert.match(picker, /chatApi\.getSessionCommands\(sessionId\)/)
  assert.match(picker, /onCommandSelect\(selected\.invocation/)
  assert.match(picker, /kind: 'tool'/)
  assert.match(picker, /callableToolNames\.has\(capability\.name\)/)
  assert.match(picker, /<Tabs\s+value=\{category\}/)
  assert.match(picker, /value="all"/)
  assert.match(picker, /value="prompt"/)
  assert.match(picker, /value="skill"/)
  assert.match(picker, /value="tool"/)
  assert.match(picker, /value="workflow"/)
  assert.match(dialog, /overlayClassName/)
  assert.match(picker, /chat-resource-body[^"\n]*min-h-0[^"\n]*overflow-hidden/)
  assert.match(picker, /chat-resource-list[^"\n]*\[flex:1_1_0\][^"\n]*overflow-y-auto/)
  assert.match(picker, /chat-resource-list[^"\n]*\[touch-action:pan-y\]/)
  assert.match(picker, /chat-resource-tabs[^"\n]*data-state='active'/)
  // 资源选择在输入框“+”菜单里（规格 v2 第 2.5 节），运行中同样可用。
  assert.match(
    focusSession,
    /data-composer-plus-item="resource"\s+onSelect=\{\(\) => setResourcePickerOpen\(true\)\}/,
  )
  assert.doesNotMatch(
    focusSession.match(/data-composer-plus-item="resource"[\s\S]*?<\/DropdownMenuItem>/)?.[0] || '',
    /disabled=/,
  )
})

test('mobile chat sends with Enter and keeps the fixed composer toolbar on touch', async () => {
  const [dock, focusSession, pageHeader, zhBoard, enBoard] = await Promise.all([
    readFile('src/features/chat/use-chat-dock.ts', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/components/layout/PageHeader.tsx', 'utf8'),
    readFile('src/locales/zh-CN/board.json', 'utf8'),
    readFile('src/locales/en-US/board.json', 'utf8'),
  ])
  const zh = JSON.parse(zhBoard)
  const en = JSON.parse(enBoard)

  assert.match(dock, /if \(compactDock \|\| panel\.group\.size <= 1\) return \[closeItem\]/)
  assert.doesNotMatch(dock, /disabled: compactDock/)
  assert.match(focusSession, /const mobileApp = useIsMobileApp\(\)/)
  assert.match(
    focusSession,
    /!composing && matchesShortcut\(event\.nativeEvent, shortcuts\.sendMessage\)/,
  )
  assert.match(focusSession, /useShortcutStore\(\(state\) => state\.bindings\)/)
  // IME 组词双保险：Chromium 看 isComposing，Mac WebKit 看自行跟踪的 imeComposingRef。
  assert.match(focusSession, /event\.nativeEvent\.isComposing \|\| imeComposingRef\.current/)
  assert.match(focusSession, /event\.currentTarget\.form\?\.requestSubmit\(\)/)
  assert.match(focusSession, /const mobileLayout = mobileApp \|\| phoneViewport/)
  assert.match(
    focusSession,
    /enterKeyHint=\{\s*mobileLayout && shortcuts\.sendMessage === 'Enter' \? 'send' : 'enter'\s*\}/,
  )
  // 触屏运行中的占位文字不写键位（没有 ⌥）：点发送排队、长按发送插话（规格 7.2）。
  assert.match(focusSession, /\? mobileLayout\s*\? t\('board:composer\.placeholderRunningTouch'\)/)
  assert.match(focusSession, /data-mobile-composer-input=\{mobileLayout \|\| undefined\}/)
  // 工具行固定（[+] 盾牌 模型 发送），不再按宽度把工具挪进收纳区；手机上同样是 44px 触控目标。
  assert.doesNotMatch(focusSession, /toolbarAllocation|ComposerToolTray|useComposerToolbarCapacity/)
  assert.match(focusSession, /onLongPress=\{phonePage && streaming \? submitSteer : undefined\}/)
  assert.match(pageHeader, /page === 'chat'[\s\S]*?!min-h-12[\s\S]*?max-\[650px\]:!flex-nowrap/)
  assert.doesNotMatch(zh['composer.placeholderRunningTouch'], /Shift|Enter|Alt|⌥/)
  assert.doesNotMatch(en['composer.placeholderRunningTouch'], /Shift|Enter|Alt|⌥/)
})

test('chat Composer discovers Runtime Slash commands without exposing templates to the client', async () => {
  const [menu, focusSession, chatApi, routes] = await Promise.all([
    readFile('src/features/chat/ComposerCommandMenu.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
    readFile('runtime/http/routes/sessions-runtime.mjs', 'utf8'),
  ])

  assert.match(menu, /chatApi\s*\.getSessionCommands\(sessionId\)/)
  assert.match(menu, /event\.key === 'ArrowDown'/)
  assert.match(menu, /event\.key === 'Tab'/)
  assert.match(menu, /command\.invocation/)
  assert.doesNotMatch(menu, /composer-command-trigger/)
  assert.doesNotMatch(menu, /command\.content|command\.filePath/)
  assert.match(focusSession, /<ComposerCommandMenu/)
  assert.match(chatApi, /getSessionCommands:/)
  assert.match(routes, /\/api\/sessions\/:sessionId\/commands/)
})

test('scheduled tasks use structured prompt or workflow targets across every form', async () => {
  const [schedules, facade, runtime] = await Promise.all([
    readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
    readFile('runtime/runtime/agent-runtime-facade.mjs', 'utf8'),
    readFile('runtime/runtime/agent-runtime.mjs', 'utf8'),
  ])

  assert.equal(schedules.match(/<ScheduleTargetFields/g)?.length, 3)
  assert.match(schedules, /type ScheduleTargetType = 'prompt' \| 'workflow'/)
  assert.match(schedules, /workflowInputs: Record<string, unknown>/)
  assert.match(schedules, /scheduleTargetValid\(/)
  assert.match(schedules, /targetType === 'prompt'/)
  assert.match(facade, /\.filter\(\(workflow\) => workflow\.status === 'published'\)/)
  assert.match(facade, /\.map\(\(\{ id, name, description, revision, inputs \}\)/)
  assert.match(facade, /createScheduleWorkflowAdapter[\s\S]*?list:[\s\S]*?run:[\s\S]*?getRun:/)
  assert.match(runtime, /workflows: createScheduleWorkflowAdapter\(this\.workflows\)/)
  assert.ok(
    runtime.indexOf('await this.workflows.init()') < runtime.indexOf('await this.schedules.init()'),
  )
})

test('historical sessions transition through loading before resolving the welcome or transcript', async () => {
  const [dock, session, transcript, catalog, styles] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(dock, /transcriptLoadState=/)
  assert.match(dock, /state\.loaded \? 'ready'/)
  assert.match(dock, /state\.loading \|\| !state\.error \? 'loading' : 'error'/)
  assert.match(
    session,
    /hasConversation = transcriptLoadState !== 'ready' \|\| messages\.length > 0/,
  )
  assert.match(session, /transcriptLoadState=\{transcriptLoadState\}/)
  assert.match(transcript, /data-pisper-transcript-state=\{transcriptLoadState\}/)
  assert.match(transcript, /transcriptLoadState === 'loading'/)
  const emptyReady = /transcriptLoadState === 'ready'\s*&&\s*!messages\.length/
  assert.match(transcript, emptyReady)
  assert.ok(transcript.indexOf("transcriptLoadState === 'loading'") < transcript.search(emptyReady))
  assert.match(catalog, /loaded: true/)
  assert.match(transcript, /session-history-loading[^"\n]*transcript-stage-enter/)
  assert.match(transcript, /transcript-reveal-enter/)
  assert.match(styles, /@keyframes transcript-stage-enter/)
})

test('session labels are searchable from Ctrl K and resolve through virtualized chat history', async () => {
  const [
    palette,
    app,
    events,
    transcript,
    virtualTranscript,
    treeDialog,
    treeBrowser,
    treeNodes,
    treeModel,
    chatMessage,
    chatPage,
    chatApi,
    routes,
    treeActions,
  ] = await Promise.all([
    readFile('src/components/layout/AppOverlays.tsx', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/chat/events.ts', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
    readFile('src/features/chat/SessionTreeDialog.tsx', 'utf8'),
    readFile('src/features/chat/session-tree-browser.tsx', 'utf8'),
    readFile('src/features/chat/session-tree-nodes.tsx', 'utf8'),
    readFile('src/features/chat/session-tree-model.ts', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
    readFile('runtime/http/routes/sessions-runtime.mjs', 'utf8'),
    readFile('src/features/chat/session-tree-actions.ts', 'utf8'),
  ])

  // 仍经领域 API 搜索；新增 AbortSignal 用于查询切换/卸载取消，不固定参数个数。
  assert.match(palette, /chatApi\.searchSessionTreeLabels\(/)
  assert.match(palette, /label\.sessionName/)
  assert.match(palette, /sessionCreated \|\| label\.sessionModified/)
  assert.match(palette, /label\.nodeTimestamp/)
  assert.match(app, /targetEntryId && !targetActive/)
  // 规格 v2 第 4.5 节老问题 2：⌘K 打开另一条分支上的标记不再悄悄切换分支。App 只按需加载会话树操作
  // （不把 chat-api 拉进启动包），由它先确认、再导航、再打开，并在提示里给撤销。
  assert.match(app, /import\(\s*'@\/features\/chat\/session-tree-actions'\s*\)/)
  assert.match(app, /confirm: appDialog\.confirm/)
  assert.doesNotMatch(app, /navigateSessionTreeTarget/)
  const switchBody = treeActions.slice(
    treeActions.indexOf('export async function switchToTurnOnOtherBranch'),
  )
  assert.ok(
    switchBody.indexOf('await confirm(otherBranchConfirmOptions(t))') <
      switchBody.indexOf('navigateSessionTreeTarget(sessionId, entryId)'),
    'confirm before switching the branch',
  )
  assert.match(switchBody, /return false/)
  assert.match(switchBody, /notifyTreeNavigation\(/)
  assert.match(treeActions, /label: deps\.t\('chat:sessionTree\.undo'\)/)
  assert.match(palette, /pendingEntryId/)
  assert.match(palette, /appOverlays\.locatingLabel/)
  assert.match(events, /STORAGE_KEYS\.sessionMessageTarget/)
  assert.match(transcript, /message\.turnBoundaryEntryId === targetEntryId/)
  assert.match(transcript, /if \(hasOlder\)/)
  assert.match(virtualTranscript, /virtualizer\.scrollToIndex\(targetIndex/)
  assert.match(virtualTranscript, /data-pisper-target-entry/)
  // 会话树 UI 拆分到 session-tree-* 子模块（仍由懒加载的对话框独占引入，
  // 保持在独立 chunk 内）；断言按新布局分布到对应文件。
  assert.match(treeDialog, /from '@\/features\/chat\/session-tree-browser'/)
  assert.match(treeDialog, /from '@\/features\/chat\/session-tree-model'/)
  assert.match(treeDialog, /node\.branchPoint/)
  assert.match(treeNodes, /session-tree-children/)
  assert.match(treeBrowser, /sessionTree\.searchPlaceholder/)
  assert.match(treeBrowser, /chat-resource-search[^"\n]*\[&_input\]:\[outline:0\]!/)
  assert.match(treeDialog, /showCloseButton=\{false\}/)
  assert.match(treeDialog, /aria-label=\{t\('common:ui\.closeDialog'\)\}/)
  assert.match(treeDialog, /onClick=\{onClose\}/)
  assert.match(treeDialog, /buildDisplayTree\(data\?\.nodes \|\| \[\]\)/)
  assert.doesNotMatch(
    treeModel.match(/const conversationKinds = new Set\(\[[^\]]+\]\)/)?.[0] || '',
    /'tool(?:-call)?'/,
  )
  assert.match(treeModel, /nonDerivableKinds = new Set\(\['tool', 'tool-call'\]\)/)
  assert.match(treeDialog, /canDeriveSelected/)
  // 三个页：分支 / 标记 / 全部标记（规格 4.5）；原“对话”“全部”两页并进“分支”页（“显示全部条目”勾选框）。
  assert.doesNotMatch(treeDialog, /branchRelated/)
  assert.deepEqual(
    [...treeBrowser.matchAll(/<TabsTrigger value="([a-z]+)"/g)].map((match) => match[1]),
    ['branches', 'labeled', 'marks'],
  )
  assert.match(treeBrowser, /data-session-tree-all-entries/)
  assert.match(treeModel, /showAllEntries \|\| conversationKinds\.has\(node\.kind\)/)
  assert.match(treeDialog, /navigateSessionTree\(sessionId, selected\.id, summarize\)/)
  assert.match(treeDialog, /navigateSessionTreeTarget\(mark\.sessionId, mark\.entryId\)/)
  // 会话树“全部标记”里另一条分支上的标记同样先就地确认（data-session-tree-mark-confirm）。
  assert.match(
    treeDialog,
    /if \(!mark\.active && !confirmed\) \{\s+setConfirmingMark\(mark\)\s+return/,
  )
  assert.match(treeBrowser, /data-session-tree-mark-confirm/)
  // 老问题 4（已复现：导航父会话到派生点会把它的当前分支退回去）：“返回原对话”只打开并定位，不导航父会话。
  const openRelated = treeDialog.slice(
    treeDialog.indexOf('const openRelatedSession'),
    treeDialog.indexOf('const navigate = async'),
  )
  assert.match(openRelated, /requestSessionSelection\(target\.id, 'open', targetEntryId\)/)
  assert.doesNotMatch(openRelated, /navigateSessionTree/)
  // 老问题 5：“派生自”横幅是链接，打开父会话并定位到派生点，同样不导航父会话的树。
  assert.match(transcript, /data-pisper-parent-session=\{lineage\.parentSessionId\}/)
  assert.match(transcript, /href=\{lineageHref\(lineage\.parentSessionId\)\}/)
  assert.doesNotMatch(transcript, /navigateSessionTree/)
  assert.match(chatMessage, /data-pisper-label-entry/)
  assert.match(chatMessage, /data-pisper-derive-entry/)
  assert.match(chatMessage, /data-pisper-child-entry/)
  assert.match(chatMessage, /onBranchFromHere\(boundaryEntryId\)/)
  assert.match(chatMessage, /setSessionTreeLabel\(sessionId, entryId, label\)/)
  // 老问题 1、6：标记与导航的响应（带整棵树）立刻写回共享的会话树摘要；转录读标记名，顶栏读分支数。
  assert.match(chatMessage, /applySessionTreeResponse\(sessionId, tree\)/)
  assert.match(chatMessage, /useTurnLabel\(sessionId,/)
  assert.match(chatMessage, /data-turn-label=/)
  assert.match(chatPage, /navigateSessionTree\(session\.id, boundaryEntryId, false\)/)
  assert.match(chatPage, /applySessionTreeResponse\(session\.id, result\)/)
  assert.match(chatPage, /notifyTreeNavigation\(session\.id, result,/)
  assert.match(treeDialog, /applySessionTreeResponse\(sessionId, next\)/)
  assert.match(chatApi, /navigateSessionTreeTarget:/)
  assert.match(chatApi, /includeTree: false/)
  // 图标按钮与文字菜单的可达性由 workbench 消息交互用例验证，不绑定 Tooltip 数量。
  assert.doesNotMatch(
    chatMessage,
    /title=\{t\('chat:chatMessage\.(?:labelThisTurn|deriveFromHere)'\)\}/,
  )
  assert.match(routes, /path: '\/api\/session-labels'/)
  assert.match(routes, /input\.includeTree === false/)
})

test('settings keep an in-page section navigation without the application sidebar and stay reachable in the mobile shell', async () => {
  const [app, sectionNav, more, mobileNavigation, settingsNavigation, styles] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/SettingsSectionNav.tsx', 'utf8'),
    readFile('src/app/MorePage.tsx', 'utf8'),
    readFile('src/components/layout/MobileNavigation.tsx', 'utf8'),
    readFile('src/app/settings-navigation.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.doesNotMatch(app, /SettingsShell/)
  assert.match(
    app,
    /<div[\s\S]*?className=\{`page-content[\s\S]*?page-\$\{page\}[\s\S]*?key=\{page\}[\s\S]*?<Outlet/,
  )
  // 手机用横滑分区条；手机 App 平板（≥960×640，规格 7.6）与桌面一样用左侧分区导航。
  assert.match(
    app,
    /clientLoaded && mobileLayout && SETTINGS_PAGES\.has\(page\) && settingsOwned && !tabletApp/,
  )
  assert.match(app, /<MobileSettingsNavigation[\s\S]*?mobileApp=\{mobileApp\}/)
  // 一级导航始终是应用侧栏（手机外壳为底部标签栏）；设置整页换侧栏内容是 A3，这一期设置分区导航
  // 仍在内容区左侧。当前页点亮哪个入口只由 activeShellTab(page, settingsOwned) 决定
  // （组件工作室归“功能”）。会话页与主页隐藏手机标签栏（规格 7.3）。
  assert.match(app, /const shellTab = activeShellTab\(page, settingsOwned\)/)
  assert.match(app, /<ShellSidebar\s+active=\{sidebarActive\}/)
  assert.match(app, /<ShellTabBar\s+hidden=\{page === 'session'\}\s+activeTab=\{shellTab\}/)
  assert.doesNotMatch(app, /<WorkbenchNavigation\b|<WorkbenchHeader\b|<AppSidebar\b/)
  // 桌面与手机 App 平板的设置页在内容区左侧有自己的分区导航；手机仍用横滑分区条。
  assert.match(app, /const tabletApp = useIsTabletViewport\(\) && mobileApp/)
  assert.match(app, /const phoneLayout = mobileLayout && !tabletApp/)
  assert.match(app, /const settingsLayout = settingsOwned && !phoneLayout/)
  assert.match(
    app,
    /\{settingsLayout && \([\s\S]*?<SettingsSectionNav\s+page=\{page\}\s+configSection=\{configSection\}\s+mobileApp=\{mobileApp\}\s+onNavigate=\{navigateSettings\}/,
  )
  assert.match(sectionNav, /getSettingsNavigation\(t, \{ mobileApp, capabilities \}\)/)
  assert.match(sectionNav, /aria-label=\{t\('config:settingsShell\.settingsNavigation'\)\}/)
  assert.match(sectionNav, /aria-current=\{active \? 'page' : undefined\}/)
  // 功能页入口在“更多”（v2 改名“功能”）启动台，按 Runtime 能力与客户端形态裁剪；手机标签栏与 v2 桌面
  // 都没有“组件”入口，组件页作为“功能”里的磁贴出现。
  assert.match(
    more,
    /getMoreNavigation\(t, \{ capabilities, mobileApp, components, componentsTile: true \}\)/,
  )
  assert.match(more, /const phone = usePhoneShell\(\)/)
  assert.match(mobileNavigation, /getSettingsNavigation\(t, \{ mobileApp, capabilities \}\)/)
  assert.match(mobileNavigation, /useRuntimeCapabilitiesStore/)
  assert.doesNotMatch(app, /<MobilePrimaryNavigation/)
  assert.match(mobileNavigation, /data-mobile-navigation="settings"/)
  assert.match(settingsNavigation, /getSettingsNavigation/)
  assert.match(settingsNavigation, /config:mobile-server[\s\S]*config:notifications/)
  assert.doesNotMatch(settingsNavigation, /mobile-device|mobileDevice/)
  assert.doesNotMatch(styles, /\.settings-shell|\.settings-nav|\.settings-content/)
})

test('mobile shell keeps navigation in the viewport and model settings retain responsive navigation/detail panels', async () => {
  const [app, models, connectionList, wizard, apiKeyList] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/config/ModelsSettings.tsx', 'utf8'),
    readFile('src/features/config/ConnectionList.tsx', 'utf8'),
    readFile('src/features/config/QuickSetupWizard.tsx', 'utf8'),
    readFile('src/features/config/ApiKeyInput.tsx', 'utf8'),
  ])

  assert.match(
    app,
    /app-shell[^"\n]*\[&\[data-mobile-app\]\]:h-\[100dvh\][^"\n]*\[&\[data-mobile-app\]\]:overflow-hidden/,
  )
  assert.match(app, /app-body[^"\n]*min-h-0[^"\n]*flex-1[^"\n]*overflow-hidden/)
  assert.equal(app.match(/data-mobile-app=\{mobileLayout \|\| undefined\}/g)?.length, 2)
  // 模型设置页为单列扁平结构：摘要 + 发现 + 连接列表 + 运行策略 + 视觉生成
  assert.doesNotMatch(models, /!grid-cols-/)
  assert.match(models, /<ProviderWorkbench/)
  const workbench = await readFile('src/features/config/ProviderWorkbench.tsx', 'utf8')
  // 布局与窄屏可读性由 smoke-pisper-ui 行为验收；这里保护统一连接工作台的职责边界。
  assert.match(workbench, /data-model-provider-split-panel/)
  assert.match(workbench, /<nav[\s\S]*?aria-label=\{t\('config:configPage.connections'\)\}/)
  assert.match(workbench, /<ProviderConnectionEditor/)
  assert.match(workbench, /<ProviderModelEditor/)
  assert.doesNotMatch(workbench, /<Tabs|<ProviderConfigModal/)
  assert.match(models, /<RuntimePolicySettings/)
  assert.match(models, /<VisualGenerationSettings/)
  assert.match(connectionList, /value=\{provider\.configured && provider\.enabled\}/)
  assert.match(connectionList, /<Badge variant="accent">/)
  // 单 Key 复用控件继续保证密码语义与禁用自动填充。
  assert.match(wizard, /<ApiKeyInput/)
  assert.match(apiKeyList, /type="password"/)
  assert.match(apiKeyList, /autoComplete="new-password"/)
})

test('mobile device permissions are requested by native operations', async () => {
  const [configPage, serverSettings, navigation, native] = await Promise.all([
    readFile('src/features/config/ConfigPage.tsx', 'utf8'),
    readFile('src/features/config/MobileServerSettings.tsx', 'utf8'),
    readFile('src/app/settings-navigation.ts', 'utf8'),
    readFile('src-tauri/src/mobile/mod.rs', 'utf8'),
  ])

  assert.doesNotMatch(configPage, /MobileDeviceSettings|mobile-device/)
  assert.doesNotMatch(serverSettings, /MobileDeviceSettings/)
  assert.doesNotMatch(navigation, /MobileDeviceSettings|mobile-device|mobileDevice/)
  assert.match(native, /request_permission\(capability\)/)
  assert.match(native, /states\.get\(capability\)/)
})

test('route code and route-specific vendor styles remain lazy', async () => {
  const [router, routeElements, main, chat, chatDock, workflows, styles, appearance] =
    await Promise.all([
      readFile('src/app/router.tsx', 'utf8'),
      readFile('src/app/route-elements.tsx', 'utf8'),
      readFile('src/main.tsx', 'utf8'),
      readFile('src/features/chat/ChatPage.tsx', 'utf8'),
      readFile('src/features/chat/ChatDock.tsx', 'utf8'),
      readFile('src/features/workflows/WorkflowsPage.tsx', 'utf8'),
      readFile('src/index.css', 'utf8'),
      readFile('src/app/ChatAppearancePage.tsx', 'utf8'),
    ])

  // 实际页面保持动态导入；组件标签直接懒加载组件管理页（旧的 ?view=widgets 反向跳到这里），
  // 不再是去设置页的重定向。
  const loaders = [...routeElements.matchAll(/export async function (\w+Route)\(/g)]
  assert.ok(loaders.length > 0)
  for (const [index, loader] of loaders.entries()) {
    const body = routeElements.slice(loader.index, loaders[index + 1]?.index)
    assert.match(body, /\bimport\(/, `${loader[1]} must import its page dynamically`)
    assert.match(body, /await (?:import\(|Promise\.all\()/, `${loader[1]} must await its page`)
    assert.doesNotMatch(body, /<Navigate\b/, `${loader[1]} must render a page, not redirect`)
    assert.match(router, new RegExp(`lazy: ${loader[1]}\\b`))
  }
  for (const route of ['boardRoute', 'sessionRoute', 'moreRoute', 'componentsRoute'])
    assert.ok(
      loaders.some((loader) => loader[1] === route),
      `${route} must be a lazy route`,
    )
  assert.ok((router.match(/lazy: \w+Route/g)?.length || 0) >= 12)
  assert.doesNotMatch(appearance, /ChatLayoutEditor|interface-chat-layout/)
  assert.doesNotMatch(chat, /useChatLayoutStore|canvasHasKind/)
  assert.doesNotMatch(router, /from '@\/features\//)
  // 第 4 期起界面设置不再嵌入组件管理（“独立组件”标签已移除，只留跳到组件页的链接）：
  // 组件管理只随组件路由懒加载，设置页不再拉取 custom-ui 的任何代码。
  assert.doesNotMatch(appearance, /@\/features\/custom-ui|CustomUiPage/)
  assert.doesNotMatch(appearance, /from '@\/features\//)
  const componentsLoader = routeElements.slice(
    routeElements.indexOf('export async function componentsRoute'),
  )
  assert.match(componentsLoader.slice(0, 200), /import\('@\/features\/custom-ui\/public'\)/)
  assert.doesNotMatch(main, /react-bits\.css|dockview\.css|@xyflow\/react\/dist\/style\.css/)
  // 桌面分屏按需加载；手机只显示所选任务，不能把 Dockview 样式拖入入口。
  assert.doesNotMatch(chat, /dockview-react\/dist\/styles\/dockview\.css/)
  // 第 3 期起聊天页不再挂载分屏视图，v2（A1）删除了 ChatDockView：Dockview 的运行时与样式不进入任何路由。
  assert.doesNotMatch(chat, /ChatDockView|dockview-react\/dist/)
  await assert.rejects(readFile('src/features/chat/ChatDockView.tsx', 'utf8'), { code: 'ENOENT' })
  assert.match(chat, /<MobileSessionPanel/)
  assert.match(chat, /singleSessionLayout: mobileLayout/)
  assert.match(chatDock, /className="mobile-session-tabs/)
  assert.match(chatDock, /role="tablist"/)
  assert.doesNotMatch(chatDock, /MOBILE_SESSION_TAB_LIMIT/)
  assert.match(chatDock, /STORAGE_KEYS\.mobileSessionTabs/)
  assert.match(chatDock, /onCreateSession/)
  assert.doesNotMatch(styles, /dockview/)
  assert.match(workflows, /import '@xyflow\/react\/dist\/style\.css'/)
  assert.match(styles, /@import "tailwindcss" source\(none\);/)
  assert.match(styles, /@source "\.\/\*\*\/\*\.\{ts,tsx\}";/)
})
