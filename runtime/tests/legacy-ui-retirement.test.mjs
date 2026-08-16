import assert from 'node:assert/strict'
import { access, readFile, readdir } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true })
  const nested = await Promise.all(
    entries.map(async (entry) => {
      const target = path.join(directory, entry.name)
      if (entry.isDirectory()) return sourceFiles(target)
      return /\.(?:ts|tsx)$/.test(entry.name) ? [target] : []
    }),
  )
  return nested.flat()
}

test('business UI no longer depends on the legacy primitive barrel', async () => {
  await assert.rejects(access('src/components/ui.tsx'))

  const files = await sourceFiles('src')
  const sources = await Promise.all(files.map(async (file) => [file, await readFile(file, 'utf8')]))
  const legacyImports = sources
    .filter(([, source]) => /from ['"]@\/components\/ui['"]/.test(source))
    .map(([file]) => file)

  assert.deepEqual(legacyImports, [])
})

test('app primitives compose shadcn controls and share project Tailwind tokens', async () => {
  const [primitives, settings, styles, schedules] = await Promise.all([
    readFile('src/components/ui/app-primitives.tsx', 'utf8'),
    readFile('src/features/config/settings-primitives.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
  ])

  for (const primitive of ['badge', 'card', 'switch', 'tabs']) {
    assert.match(primitives, new RegExp(`@/components/ui/${primitive}`))
  }
  assert.match(settings, /from '@\/components\/ui\/app-primitives'/)
  assert.match(styles, /--background: var\(--bg\)/)
  assert.match(styles, /--card: var\(--panel\)/)
  assert.match(styles, /--ring: var\(--focus\)/)
  assert.match(styles, /--shadow-surface: var\(--sh-surface\)/)
  assert.match(
    schedules,
    /split-list-detail[^"\n]*max-\[650px\]:\[\.split-list-detail&\]:grid-cols-\[1fr\]/,
  )
  assert.match(schedules, /max-\[650px\]:\[\.split-list-detail&\]:\[overflow-x:visible\]/)
  assert.doesNotMatch(styles, /\.panel(?:\W|$)/)
  assert.doesNotMatch(styles, /\.toast(?:\W|$)/)
})

test('desktop shell fills the WebView through the root percentage height chain', async () => {
  const [styles, app] = await Promise.all([
    readFile('src/index.css', 'utf8'),
    readFile('src/App.tsx', 'utf8'),
  ])
  assert.match(styles, /html, body, #root \{[^}]*height: 100%;/)
  assert.match(app, /app-shell[^"\n]*h-full/)
  assert.doesNotMatch(app, /app-shell\s+h-\[100dvh\]/)
})

test('modal surfaces stay scrollable within low-height viewports', async () => {
  const [styles, dialog, alertDialog, overlays, assets] = await Promise.all([
    readFile('src/index.css', 'utf8'),
    readFile('src/components/ui/dialog.tsx', 'utf8'),
    readFile('src/components/ui/alert-dialog.tsx', 'utf8'),
    readFile('src/components/layout/AppOverlays.tsx', 'utf8'),
    readFile('src/features/assets/AssetsPage.tsx', 'utf8'),
  ])

  assert.match(assets, /modal-backdrop[^"\n]*overflow-y-auto/)
  assert.match(
    overlays,
    /className="modal[^"\n]*max-h-\[calc\(100dvh_-_40px\)\][^"\n]*overflow-y-auto/,
  )
  assert.match(overlays, /max-\[650px\]:max-h-\[calc\(100dvh_-_16px\)\]/)
  assert.doesNotMatch(styles, /\.directory-browser/)
  assert.match(dialog, /max-h-\[calc\(100dvh-2rem\)\].*overflow-y-auto.*overscroll-contain/)
  assert.match(alertDialog, /max-h-\[calc\(100dvh-2rem\)\].*overflow-y-auto.*overscroll-contain/)
})

test('toast and app dialogs use integrated Radix and shadcn surfaces', async () => {
  const [app, toast, appDialog, overlays] = await Promise.all([
    readFile('src/App.tsx', 'utf8'),
    readFile('src/components/ui/toast.tsx', 'utf8'),
    readFile('src/components/layout/AppDialog.tsx', 'utf8'),
    readFile('src/components/layout/AppOverlays.tsx', 'utf8'),
  ])

  assert.match(toast, /Toast as ToastPrimitive/)
  assert.match(toast, /ToastPrimitive\.Provider/)
  assert.match(toast, /ToastPrimitive\.Viewport/)
  assert.doesNotMatch(toast, /sonner|next-themes/)
  assert.match(app, /<ToastProvider duration=\{2800\}/)
  assert.match(app, /<ToastViewport \/>/)
  assert.doesNotMatch(app, /toastTimer|setTimeout\(\(\) => setToast/)
  assert.match(appDialog, /from '@\/components\/ui\/dialog'/)
  assert.match(overlays, /<Dialog open/)
})
