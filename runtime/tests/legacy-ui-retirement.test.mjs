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
  const [primitives, settings, styles] = await Promise.all([
    readFile('src/components/ui/app-primitives.tsx', 'utf8'),
    readFile('src/features/config/settings-primitives.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
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
    styles,
    /\.split-list-detail\.schedule-layout \{ grid-template-columns: 1fr; overflow-x: visible; \}/,
  )
  assert.doesNotMatch(styles, /\.panel(?:\W|$)/)
  assert.doesNotMatch(styles, /\.toast(?:\W|$)/)
})

test('desktop shell fills the WebView through the root percentage height chain', async () => {
  const styles = await readFile('src/index.css', 'utf8')

  const appShellRule = styles.match(/\.app-shell \{([^}]*)\}/)?.[1] || ''
  assert.match(styles, /html, body, #root \{[^}]*height: 100%;/)
  assert.match(appShellRule, /(?:^|;)\s*height: 100%;/)
  assert.doesNotMatch(appShellRule, /(?:^|;)\s*height: 100dvh;/)
})

test('modal surfaces stay scrollable within low-height viewports', async () => {
  const [styles, dialog, alertDialog] = await Promise.all([
    readFile('src/index.css', 'utf8'),
    readFile('src/components/ui/dialog.tsx', 'utf8'),
    readFile('src/components/ui/alert-dialog.tsx', 'utf8'),
  ])

  assert.match(styles, /\.modal-backdrop \{[^}]*overflow-y: auto;/)
  assert.match(styles, /\.modal \{[^}]*max-height: calc\(100dvh - 40px\);[^}]*overflow-y: auto;/)
  assert.match(styles, /\.modal \{ max-height: calc\(100dvh - 16px\); \}/)
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
