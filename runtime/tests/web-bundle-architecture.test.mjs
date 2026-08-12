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

test('workflow notification targets stay selectable before channel setup', async () => {
  const inspector = await readFile('src/features/workflows/WorkflowNodeInspector.tsx', 'utf8')
  const notificationSwitch = inspector.match(
    /<Switch\s+size="sm"[\s\S]*?onCheckedChange=\{\(\) => onToggleNotification\(id\)\}/,
  )?.[0]

  assert.ok(notificationSwitch)
  assert.doesNotMatch(notificationSwitch, /disabled=/)
  assert.match(notificationSwitch, /catalog\.notificationTargets\[id\]\?\.enabled/)
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
