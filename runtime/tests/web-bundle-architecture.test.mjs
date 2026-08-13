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

test('workflow notifications separate system permission from external channel setup', async () => {
  const [inspector, editor, switchPrimitive] = await Promise.all([
    readFile('src/features/workflows/WorkflowNodeInspector.tsx', 'utf8'),
    readFile('src/features/workflows/useWorkflowEditor.ts', 'utf8'),
    readFile('src/components/ui/switch.tsx', 'utf8'),
  ])
  const notificationSwitch = inspector.match(
    /<Switch[\s\S]*?onCheckedChange=\{\(\) => void onToggleNotification\(id\)\}/,
  )?.[0]

  assert.ok(notificationSwitch)
  assert.match(
    inspector,
    /id === 'browser'[\s\S]*?systemNotificationAvailable[\s\S]*?catalog\.notificationTargets\[id\]\?\.enabled/,
  )
  assert.match(
    inspector,
    /systemNotificationPermission === 'default' \|\| systemNotificationPermission === 'granted'/,
  )
  assert.match(notificationSwitch, /disabled=\{!targetEnabled\}/)
  assert.doesNotMatch(notificationSwitch, /size="sm"/)
  assert.match(inspector, /noExternalNotificationChannelsEnabled/)
  assert.match(inspector, /systemNotificationPermissionRequired/)
  assert.match(inspector, /onOpenChannels/)
  assert.match(inspector, /onOpenSystemNotificationSettings/)
  assert.match(editor, /target !== 'browser'/)
  assert.match(editor, /requestBrowserNotificationPermission/)
  assert.match(editor, /\/api\/settings\/notifications\/browser/)
  assert.match(editor, /notificationTargets\.filter/)
  assert.match(switchPrimitive, /inline-flex min-h-0! shrink-0/)
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
