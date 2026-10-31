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
  const [primitives, settings, styles, schedules, featurePage] = await Promise.all([
    readFile('src/components/ui/app-primitives.tsx', 'utf8'),
    readFile('src/features/config/settings-primitives.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/features/schedules/SchedulesPage.tsx', 'utf8'),
    readFile('src/components/app/FeaturePage.tsx', 'utf8'),
  ])

  for (const primitive of ['badge', 'card', 'switch', 'tabs']) {
    assert.match(primitives, new RegExp(`@/components/ui/${primitive}`))
  }
  assert.match(settings, /from '@\/components\/ui\/app-primitives'/)
  assert.match(styles, /--background: var\(--bg\)/)
  assert.match(styles, /--card: var\(--panel\)/)
  assert.match(styles, /--ring: var\(--focus\)/)
  assert.match(styles, /--shadow-surface: var\(--sh-surface\)/)
  // 定时任务改用功能页模板（A6 第 3 批）：任务是可展开的列表行，不再叠列表 / 详情两栏。
  // ListDetailLayout 原语在 A6 末已没有调用方并删除；取代它“单栏 / 双栏切换不横向溢出”的保护：
  // 功能页模板只有一个纵向滚动区（禁止横向滚动），展开的详情是行下方的 region、受名称按钮 aria-controls 控制。
  assert.match(schedules, /<FeaturePage\b/)
  assert.match(schedules, /<FeatureRow\b[\s\S]*?onExpandedChange=/)
  assert.doesNotMatch(schedules, /<ListDetailLayout\b/)
  await assert.rejects(readFile('src/components/app/ListDetailLayout.tsx', 'utf8'))
  assert.match(featurePage, /data-feature-scroller[\s\S]*?overflow-x-hidden overflow-y-auto/)
  assert.match(featurePage, /aria-controls=\{expanded \? detailId : undefined\}/)
  assert.match(featurePage, /id=\{detailId\}\s+role="region"/)
  assert.doesNotMatch(schedules, /!grid-cols-\[310px_minmax\(0,1fr\)\]/)
  assert.doesNotMatch(schedules, /minmax\(420px,1fr\)/)
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
    readFile('src/features/assets/AssetPreviewDialog.tsx', 'utf8'),
  ])

  // 资产预览固定对话框外框，只让正文区域滚动。
  assert.match(assets, /scrollMode="body"/)
  assert.match(assets, /<DialogBody\b/)
  assert.match(
    dialog,
    /function DialogBody[\s\S]*?min-h-0 flex-1 overflow-y-auto overscroll-contain/,
  )
  // 命令面板（AppOverlays 里只剩它；页头的“快速创建”假表单已删）：结果列表按可视高度封顶并独立滚动，
  // 矮窗口里弹层顶部不低于 16px（取代原快速创建对话框的 100dvh 上限断言）。
  assert.doesNotMatch(overlays, /export function QuickCreate\b/)
  assert.match(overlays, /role="listbox"/)
  assert.match(
    overlays,
    /max-h-\[min\(440px,max\(120px,calc\(90dvh-104px\)\)\)\] overflow-y-auto overscroll-contain/,
  )
  assert.match(overlays, /top-\[max\(16px,/)
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
  assert.match(app, /<ToastViewport hotkey=\{\[\]\} label=\{t\('common:ui\.notifications'\)\} \/>/)
  assert.doesNotMatch(app, /toastTimer|setTimeout\(\(\) => setToast/)
  assert.match(appDialog, /from '@\/components\/ui\/dialog'/)
  assert.match(overlays, /<Dialog open/)
})
