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
  const [terminal, styles] = await Promise.all([
    readFile('src/features/terminal/TerminalPanel.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(terminal, /WORKBENCH_RESERVED_HEIGHT = 420/)
  assert.match(terminal, /maximumTerminalHeight\(window\.innerHeight\)/)
  assert.match(styles, /\.page-content\.page-chat \{ display: flex;/)
  assert.match(styles, /\.chat-layout \{[^}]*min-height: 0;[^}]*flex: 1;/)
  assert.doesNotMatch(styles, /\.chat-layout \{[^}]*min-height: 510px;/)
})

test('desktop terminal keeps its collapsed row and follows the active color theme', async () => {
  const [terminal, styles] = await Promise.all([
    readFile('src/features/terminal/TerminalPanel.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(styles, /--terminal-bg: #f8fafc;/)
  assert.match(styles, /:root\[data-theme='dark'\][\s\S]*?--terminal-bg: #111318;/)
  assert.match(styles, /\.terminal-panel \{[^}]*flex: 0 0 35px;/)
  assert.match(
    styles,
    /\.terminal-toolbar \{[^}]*border-bottom: 1px solid var\(--terminal-border\)/,
  )
  assert.match(styles, /\.terminal-title,\.terminal-tab \{[^}]*color: var\(--terminal-muted\)/)
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
  const [picker, dialog, styles, focusSession] = await Promise.all([
    readFile('src/features/chat/ChatResourcePicker.tsx', 'utf8'),
    readFile('src/components/ui/dialog.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
  ])

  assert.match(picker, /className="chat-resource-dialog z-\[220\]/)
  assert.match(picker, /overlayClassName="z-\[220\]"/)
  assert.match(picker, /className="chat-resource-confirm"/)
  assert.match(picker, /apiJson<PluginsData>\(`\/api\/plugins\?sessionId=/)
  assert.match(picker, /chatApi\.getSessionCommands\(sessionId\)/)
  assert.match(picker, /onCommandSelect\(selected\.invocation/)
  assert.match(picker, /kind: 'tool'/)
  assert.match(picker, /callableToolNames\.has\(capability\.name\)/)
  assert.match(picker, /<Tabs value=\{category\}/)
  assert.match(picker, /value="all"/)
  assert.match(picker, /value="prompt"/)
  assert.match(picker, /value="skill"/)
  assert.match(picker, /value="tool"/)
  assert.match(picker, /value="workflow"/)
  assert.match(dialog, /overlayClassName/)
  assert.match(styles, /\.chat-resource-confirm \{[^}]*color: var\(--on-accent\)/)
  assert.match(styles, /\.chat-resource-body \{[^}]*min-height: 0;[^}]*overflow: hidden;/)
  assert.match(styles, /\.chat-resource-list \{[^}]*flex: 1 1 0;[^}]*overflow-y: auto;/)
  assert.match(styles, /\.chat-resource-list \{[^}]*touch-action: pan-y;/)
  assert.match(styles, /chat-resource-tabs[^}]*\[data-state='active'\]/)
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

test('session labels are searchable from Ctrl K and resolve through virtualized chat history', async () => {
  const [palette, app, events, transcript, virtualTranscript, treeDialog, chatMessage, routes] =
    await Promise.all([
      readFile('src/components/layout/AppOverlays.tsx', 'utf8'),
      readFile('src/App.tsx', 'utf8'),
      readFile('src/features/chat/events.ts', 'utf8'),
      readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
      readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
      readFile('src/features/chat/SessionTreeDialog.tsx', 'utf8'),
      readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
      readFile('runtime/http/routes/sessions-runtime.mjs', 'utf8'),
    ])

  assert.match(palette, /searchSessionTreeLabels\(keyword, 12\)/)
  assert.match(palette, /label\.sessionName/)
  assert.match(palette, /sessionCreated \|\| label\.sessionModified/)
  assert.match(palette, /label\.nodeTimestamp/)
  assert.match(app, /targetEntryId && !targetActive/)
  assert.match(app, /navigateSessionTree\(id, targetEntryId, false\)/)
  assert.match(events, /STORAGE_KEYS\.sessionMessageTarget/)
  assert.match(transcript, /message\.turnBoundaryEntryId === targetEntryId/)
  assert.match(transcript, /if \(hasOlder\)/)
  assert.match(virtualTranscript, /virtualizer\.scrollToIndex\(targetIndex/)
  assert.match(virtualTranscript, /data-pisper-target-entry/)
  assert.match(treeDialog, /node\.branchPoint/)
  assert.match(treeDialog, /session-tree-children/)
  assert.match(treeDialog, /sessionTree\.searchPlaceholder/)
  assert.match(chatMessage, /data-pisper-label-entry/)
  assert.match(chatMessage, /setSessionTreeLabel\(sessionId, entryId, label\)/)
  assert.match(routes, /path: '\/api\/session-labels'/)
})

test('settings navigation replaces the main sidebar instead of nesting in page content', async () => {
  const [app, sidebar, settingsNavigation, styles] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/layout/AppSidebar.tsx', 'utf8'),
    readFile('src/app/settings-navigation.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.doesNotMatch(app, /SettingsShell/)
  assert.match(app, /<div className=\{`page-content page-\$\{page\}`\} key=\{page\}>\s*<Outlet/)
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
  assert.ok((router.match(/lazy: \w+Route/g)?.length || 0) >= 13)
  assert.doesNotMatch(router, /from '@\/features\//)
  assert.doesNotMatch(main, /react-bits\.css|dockview\.css|@xyflow\/react\/dist\/style\.css/)
  assert.match(chat, /import 'dockview-react\/dist\/styles\/dockview\.css'/)
  assert.match(workflows, /import '@xyflow\/react\/dist\/style\.css'/)
  assert.match(styles, /@import "tailwindcss" source\(none\);/)
  assert.match(styles, /@source "\.\/\*\*\/\*\.\{ts,tsx\}";/)
})
