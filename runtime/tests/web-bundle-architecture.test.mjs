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
    /node scripts\/build-frontend\.mjs && node scripts\/check-bundle-budget\.mjs/,
  )
  assert.match(buildScript, /NODE_ENV: 'production'/)
  assert.match(buildScript, /'vite'/)
})

test('desktop terminal reattaches its xterm runtime after the panel host is remounted', async () => {
  const terminal = await readFile('src/features/terminal/TerminalPanel.tsx', 'utf8')

  assert.match(terminal, /existing\.element\.parentElement !== host/)
  assert.match(terminal, /host\.append\(existing\.element\)/)
  assert.match(terminal, /existing\.resizeObserver\.observe\(host\)/)
})

test('desktop terminals are scoped to the active chat session without stopping hidden processes', async () => {
  const [app, terminal, scope] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/terminal/TerminalPanel.tsx', 'utf8'),
    readFile('src/features/terminal/terminal-session-scope.ts', 'utf8'),
  ])

  assert.match(app, /activeSessionId=\{activeSessionId\}/)
  assert.match(app, /resolveSessionCwd=\{resolveSessionCwd\}/)
  assert.match(terminal, /sessionId: string/)
  assert.match(terminal, /const visibleTabs = visibleSessionTerminals\(tabs, activeSessionId\)/)
  assert.match(
    terminal,
    /const activeId = activeSessionTerminalId\(tabs, activeIds, activeSessionId\)/,
  )
  assert.match(
    terminal,
    /setActiveIds\(\(current\) => \(\{ \.\.\.current, \[sessionId\]: id \}\)\)/,
  )
  assert.match(terminal, /visibleTabs\.map\(\(tab\) =>/)
  assert.match(scope, /terminal\.sessionId === activeSessionId/)
  assert.doesNotMatch(terminal, /activeSessionId[\s\S]{0,100}terminalClose/)
})

test('opening the desktop terminal preserves a shrinkable chat workbench above it', async () => {
  const terminal = await readFile('src/features/terminal/TerminalPanel.tsx', 'utf8')

  assert.match(terminal, /WORKBENCH_RESERVED_HEIGHT = 420/)
  assert.match(terminal, /maximumTerminalHeight\(window\.innerHeight\)/)
  const app = await readFile('src/App.tsx', 'utf8')
  const chat = await readFile('src/features/chat/ChatPage.tsx', 'utf8')
  assert.match(app, /page-content[^"\n]*\[&\.page-chat\]:flex/)
  assert.match(chat, /chat-layout[^"\n]*min-h-0[^"\n]*flex-1/)
  assert.doesNotMatch(chat, /chat-layout[^"\n]*min-h-\[510px\]/)
})

test('desktop terminal keeps its collapsed row and follows the active color theme', async () => {
  const [terminal, styles] = await Promise.all([
    readFile('src/features/terminal/TerminalPanel.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(styles, /--terminal-bg: #f8fafc;/)
  assert.match(styles, /:root\[data-theme='dark'\][\s\S]*?--terminal-bg: #111318;/)
  assert.match(terminal, /terminal-panel[^`\n]*\[flex:0_0_35px\]/)
  assert.match(terminal, /\[border-bottom:1px_solid_var\(--terminal-border\)\]/)
  assert.match(terminal, /terminal-title[^"\n]*text-\[var\(--terminal-muted\)\]/)
  assert.match(
    styles,
    /\.terminal-xterm \.xterm \.xterm-viewport \{ background-color: var\(--terminal-bg\); \}/,
  )
  assert.match(terminal, /document\.documentElement\.dataset\.theme === 'dark'/)
  assert.match(terminal, /background: color\('--terminal-bg'/)
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
  assert.match(focusSession, /onClick=\{\(\) => setResourcePickerOpen\(true\)\}/)
  assert.doesNotMatch(
    focusSession.match(/className="resource-picker-trigger"[\s\S]*?<\/button>/)?.[0] || '',
    /disabled=\{streaming\}/,
  )
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
  assert.match(transcript, /transcriptLoadState === 'ready' && !messages\.length/)
  assert.ok(
    transcript.indexOf("transcriptLoadState === 'loading'") <
      transcript.indexOf("transcriptLoadState === 'ready' && !messages.length"),
  )
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
    chatMessage,
    chatPage,
    chatApi,
    routes,
  ] = await Promise.all([
    readFile('src/components/layout/AppOverlays.tsx', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
    readFile('src/features/chat/events.ts', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
    readFile('src/features/chat/SessionTreeDialog.tsx', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
    readFile('runtime/http/routes/sessions-runtime.mjs', 'utf8'),
  ])

  assert.match(palette, /searchSessionTreeLabels\(keyword, 12\)/)
  assert.match(palette, /label\.sessionName/)
  assert.match(palette, /sessionCreated \|\| label\.sessionModified/)
  assert.match(palette, /label\.nodeTimestamp/)
  assert.match(app, /targetEntryId && !targetActive/)
  assert.match(app, /navigateSessionTreeTarget\(id, targetEntryId\)/)
  assert.match(palette, /pendingEntryId/)
  assert.match(palette, /appOverlays\.locatingLabel/)
  assert.match(events, /STORAGE_KEYS\.sessionMessageTarget/)
  assert.match(transcript, /message\.turnBoundaryEntryId === targetEntryId/)
  assert.match(transcript, /if \(hasOlder\)/)
  assert.match(virtualTranscript, /virtualizer\.scrollToIndex\(targetIndex/)
  assert.match(virtualTranscript, /data-pisper-target-entry/)
  assert.match(treeDialog, /node\.branchPoint/)
  assert.match(treeDialog, /session-tree-children/)
  assert.match(treeDialog, /sessionTree\.searchPlaceholder/)
  assert.match(treeDialog, /buildDisplayTree\(data\?\.nodes \|\| \[\]\)/)
  assert.doesNotMatch(
    treeDialog.match(/const conversationKinds = new Set\(\[[^\]]+\]\)/)?.[0] || '',
    /'tool(?:-call)?'/,
  )
  assert.match(treeDialog, /nonDerivableKinds = new Set\(\['tool', 'tool-call'\]\)/)
  assert.match(treeDialog, /canDeriveSelected/)
  assert.doesNotMatch(treeDialog, /branchRelated|value="branches"/)
  assert.match(treeDialog, /navigateSessionTree\(sessionId, selected\.id, summarize\)/)
  assert.match(treeDialog, /navigateSessionTreeTarget\(mark\.sessionId, mark\.entryId\)/)
  assert.match(chatMessage, /data-pisper-label-entry/)
  assert.match(chatMessage, /data-pisper-derive-entry/)
  assert.match(chatMessage, /data-pisper-child-entry/)
  assert.match(chatMessage, /onBranchFromHere\(boundaryEntryId\)/)
  assert.match(chatMessage, /setSessionTreeLabel\(sessionId, entryId, label\)/)
  assert.match(chatPage, /navigateSessionTree\(session\.id, boundaryEntryId, false\)/)
  assert.match(chatApi, /navigateSessionTreeTarget:/)
  assert.match(chatApi, /includeTree: false/)
  assert.equal([...chatMessage.matchAll(/<TooltipContent side="top" sideOffset=\{6\}>/g)].length, 3)
  assert.doesNotMatch(
    chatMessage,
    /title=\{t\('chat:chatMessage\.(?:labelThisTurn|deriveFromHere)'\)\}/,
  )
  assert.match(routes, /path: '\/api\/session-labels'/)
  assert.match(routes, /input\.includeTree === false/)
})

test('settings navigation replaces the main sidebar instead of nesting in page content', async () => {
  const [app, sidebar, settingsNavigation, styles] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/AppSidebar.tsx', 'utf8'),
    readFile('src/app/settings-navigation.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.doesNotMatch(app, /SettingsShell/)
  assert.match(
    app,
    /<div[\s\S]*?className=\{`page-content[^`]*page-\$\{page\}`\}[\s\S]*?key=\{page\}[\s\S]*?<Outlet/,
  )
  assert.match(sidebar, /settingsActive \? \(/)
  assert.match(sidebar, /nav-settings-back/)
  assert.match(settingsNavigation, /getSettingsNavigation/)
  assert.doesNotMatch(styles, /\.settings-shell|\.settings-nav|\.settings-content/)
})

test('route code and route-specific vendor styles remain lazy', async () => {
  const [router, routeElements, main, chat, workflows, styles] = await Promise.all([
    readFile('src/app/router.tsx', 'utf8'),
    readFile('src/app/route-elements.tsx', 'utf8'),
    readFile('src/main.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/workflows/WorkflowsPage.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.equal(routeElements.match(/await import\(/g)?.length, 12)
  assert.ok((router.match(/lazy: \w+Route/g)?.length || 0) >= 12)
  assert.doesNotMatch(router, /from '@\/features\//)
  assert.doesNotMatch(main, /react-bits\.css|dockview\.css|@xyflow\/react\/dist\/style\.css/)
  assert.match(chat, /import 'dockview-react\/dist\/styles\/dockview\.css'/)
  assert.match(workflows, /import '@xyflow\/react\/dist\/style\.css'/)
  assert.match(styles, /@import "tailwindcss" source\(none\);/)
  assert.match(styles, /@source "\.\/\*\*\/\*\.\{ts,tsx\}";/)
})
