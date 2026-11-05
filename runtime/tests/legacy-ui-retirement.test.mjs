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

test('dropdowns and sliders use the shared shadcn controls instead of native ones', async () => {
  // 一个下拉框只有一种样子：表单用 AppSelect（触屏时它自己换原生 select），富内容选项直接组合 ui/select；
  // 滑条一律用 ui/slider。原生控件只允许出现在这两个封装里。
  const files = await sourceFiles('src')
  const sources = await Promise.all(files.map(async (file) => [file, await readFile(file, 'utf8')]))
  const allowedSelect = new Set([path.join('src', 'components', 'AppSelect.tsx')])
  const nativeSelects = sources
    .filter(([file]) => !file.startsWith(path.join('src', 'components', 'ui')))
    .filter(([file]) => !allowedSelect.has(file))
    .filter(([, source]) => /<select\b/.test(source))
    .map(([file]) => file)
  assert.deepEqual(nativeSelects, [])
  const nativeRanges = sources
    .filter(([file]) => !file.startsWith(path.join('src', 'components', 'ui')))
    .filter(([, source]) => /type=["']range["']/.test(source))
    .map(([file]) => file)
  assert.deepEqual(nativeRanges, [])
})

test('form controls share the v2 control height', async () => {
  // 规格 v2：按钮 32（小 28）、手机 44；输入框与下拉框跟按钮同高，同一行不会出现两种高度。
  const [select, input, inputGroup, appSelect] = await Promise.all([
    readFile('src/components/ui/select.tsx', 'utf8'),
    readFile('src/components/ui/input.tsx', 'utf8'),
    readFile('src/components/ui/input-group.tsx', 'utf8'),
    readFile('src/components/AppSelect.tsx', 'utf8'),
  ])
  assert.match(select, /data-\[size=default\]:h-8 /)
  assert.match(select, /position = 'popper'/)
  assert.match(input, /'h-8 max-\[650px\]:h-11 pointer-coarse:h-11 /)
  assert.match(inputGroup, /relative flex h-8 w-full/)
  assert.doesNotMatch(appSelect, /\bh-8 min-h-0\b/)
  const files = await sourceFiles('src/features')
  const overrides = []
  for (const file of files) {
    const source = await readFile(file, 'utf8')
    if (/\bh-8\.5\b/.test(source)) overrides.push(file)
  }
  assert.deepEqual(overrides, [])
})

test('new task and session composer are the same composer', async () => {
  // 新任务草稿就是会话页的 FocusSession（draft 形态），不再有另一套启动台输入框。
  const [dock, session] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
  ])
  assert.match(session, /<ModelThinkingControl\b/)
  assert.match(
    dock,
    /export function NewTaskDraftPanel[\s\S]*?<FocusSession\s+session=\{session\}\s+shortcutEnabled\s+draft=\{draft\}/,
  )
  for (const retired of [
    'src/features/board/LaunchConsole.tsx',
    'src/features/board/DispatchOverlay.tsx',
    'src/features/board/NewTaskDraft.tsx',
  ])
    await assert.rejects(access(retired), undefined, `${retired} must stay removed`)
})

test('form fields and checkboxes use the shadcn controls', async () => {
  // 原生 input 只留给 ui 组件里没有对应物的场合：文件选择、视觉隐藏的单选、取色器、
  // 以及功能页模板的页级搜索框（A6 设计，36px）。文本、数字、时间、密码一律 ui/input，
  // 复选框用 ui/checkbox 或 ui/switch。
  const files = [
    ...(await sourceFiles('src/features')),
    ...(await sourceFiles('src/components/app')),
  ]
  const allowedTypes = /type=["'](?:file|radio|color)["']/
  const offenders = []
  for (const file of files) {
    const source = await readFile(file, 'utf8')
    for (const match of source.matchAll(/<input\b([\s\S]*?)\/>/g)) {
      if (allowedTypes.test(match[1])) continue
      if (
        file.endsWith(path.join('components', 'app', 'FeaturePage.tsx')) &&
        /type="search"/.test(match[1])
      )
        continue
      offenders.push(`${file}: <input${match[1].replace(/\s+/g, ' ').slice(0, 60)}`)
    }
  }
  assert.deepEqual(offenders, [])
  const field = await readFile('src/components/ui/field.tsx', 'utf8')
  // FieldLabel 不再给原生 input/textarea 补一套旧样式（34px、无聚焦光环）。
  assert.doesNotMatch(field, /\[&_input\]:h-8\.5/)
  assert.doesNotMatch(field, /\[&_textarea\]:/)
})

test('tabs and segmented controls share one look', async () => {
  const [tabs, segmented] = await Promise.all([
    readFile('src/components/ui/tabs.tsx', 'utf8'),
    readFile('src/components/ui/segmented-control.tsx', 'utf8'),
  ])
  for (const source of [tabs, segmented]) {
    assert.match(source, /bg-surface-muted/)
    assert.match(source, /shadow-\[inset_0_0_0_1px_var\(--stroke\)\]/)
    assert.match(source, /data-(?:active|\[state=on\]):bg-solid|data-active:bg-solid/)
  }
  assert.doesNotMatch(tabs, /h-8\.5/)
})

test('a new task is the session page in draft form, without the old welcome screen', async () => {
  // 新任务一律进入 '/board/new'：会话页的样子（问候语 + 输入框），第一条消息发出时才建会话。
  // 旧欢迎页（问候语组件 + “试试这些任务”示例卡片）与没有调用方的自动创建请求链都已退役。
  for (const retired of [
    'src/features/chat/QuickPromptIdeas.tsx',
    'src/features/chat/WorkbenchGreeting.tsx',
    'src/features/chat/workbench-greeting.ts',
    'src/features/chat/WelcomeEffects.tsx',
    'src/features/chat/welcome-brand.tsx',
    'src/assets/pisper-empty-dark.svg',
  ])
    await assert.rejects(access(retired), undefined, `${retired} must stay removed`)

  const files = await sourceFiles('src')
  const sources = await Promise.all(files.map(async (file) => [file, await readFile(file, 'utf8')]))
  const offenders = sources
    .filter(([, source]) =>
      /QuickPromptIdeas|WorkbenchGreeting|agent-welcome|requestSessionCreation|SESSION_CREATE_REQUESTED_EVENT/.test(
        source,
      ),
    )
    .map(([file]) => file)
  assert.deepEqual(offenders, [])

  const [transcript, shell, chatPage, focus] = await Promise.all([
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/app/SessionShell.tsx', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
  ])
  // 真实会话没有消息时只给一句空状态；草稿的转录区是问候语。
  assert.match(transcript, /<InlineEmptyState\s+compact/)
  assert.match(focus, /emptyState=\{draft \? <NewTaskGreeting \/> : emptyState\}/)
  // '/board/new' 由会话外壳注入看板一侧的草稿契约，聊天页渲染草稿面板；发出时先建会话再由本页发送。
  assert.match(shell, /const newTaskDraft = newRoute \? draftChrome : undefined/)
  assert.match(
    chatPage,
    /routeSessionId === NEW_SESSION_ROUTE_ID && pageChrome\?\.newTaskDraft \? \(\s*<NewTaskDraftPanel draft=\{pageChrome\.newTaskDraft\} \/>/,
  )
  assert.match(
    chatPage,
    /adoptCreatedSession\(prepared\.session\)[\s\S]{0,200}openSession\(prepared\.session\.id\)[\s\S]{0,200}return sendPrompt\(/,
  )
  // 会话页不注册页面主操作：模型引导“去设置”排队的主操作不会落到这里建空会话。
  assert.doesNotMatch(chatPage, /usePagePrimaryAction|registerPrimaryAction/)
  assert.doesNotMatch(shell, /registerPrimaryAction/)
})

test('feature and app modules are all reachable from another source file', async () => {
  // 退役界面要连同模块一起删掉，不留“没人引用、只剩测试在读”的文件（基础组件与注册表组件除外）。
  const files = (await sourceFiles('src')).filter(
    (file) =>
      /^src\/(features|app|components|hooks|lib|stores)\//.test(file.split(path.sep).join('/')) &&
      !/^src\/components\/(ui|ai-elements)\//.test(file.split(path.sep).join('/')) &&
      !file.endsWith('.d.ts'),
  )
  const all = await sourceFiles('src')
  const sources = await Promise.all(all.map(async (file) => [file, await readFile(file, 'utf8')]))
  const unreachable = files.filter((file) => {
    const normalized = file.split(path.sep).join('/')
    const base = normalized.replace(/\.(tsx?)$/, '')
    const name = base.split('/').pop()
    const alias = base.replace(/^src\//, '@/')
    const escaped = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    const reference = new RegExp(
      `['"\`](${escaped(alias)}|\\.{1,2}/(?:[^'"\`]*/)?${escaped(name)})(?:\\.tsx?)?['"\`]`,
    )
    return !sources.some(([other, source]) => other !== file && reference.test(source))
  })
  assert.deepEqual(unreachable, [])
})

test('the old memory page stays retired: no star map, no review queue', async () => {
  // 星图和候选箱连同资源一起删掉（2026-11-05，用户：“老记忆页面删除”“记忆不需要确认”）。
  await assert.rejects(access('src/features/memory/MemoryGalaxy.tsx'))
  await assert.rejects(access('public/memory-galaxy-background.webp'))
  const css = await readFile('src/index.css', 'utf8')
  assert.doesNotMatch(css, /galaxy|--g-star|--g-concept/)
  const memoryFiles = (await sourceFiles('src/features/memory')).filter(
    (file) => !/\.d\.ts$/.test(file),
  )
  for (const file of memoryFiles) {
    const source = await readFile(file, 'utf8')
    assert.doesNotMatch(source, /\/api\/memory\/candidates|\/api\/settings\/memory/, file)
    assert.doesNotMatch(source, /autoApprove|MemoryCandidate/, file)
  }
  for (const locale of ['zh-CN', 'en-US']) {
    const messages = JSON.parse(await readFile(`src/locales/${locale}/memory.json`, 'utf8'))
    assert.deepEqual(
      Object.keys(messages).filter((key) => /candidate|autoApprove|draft|galaxy/i.test(key)),
      [],
    )
  }
  // 产品文案里不再有星系隐喻。
  for (const directory of ['src/locales/zh-CN', 'src/app', 'src/features']) {
    const entries = await readdir(directory, { withFileTypes: true, recursive: true })
    for (const entry of entries) {
      if (!entry.isFile() || !/\.(?:json|ts|tsx)$/.test(entry.name)) continue
      const file = path.join(entry.parentPath, entry.name)
      assert.doesNotMatch(await readFile(file, 'utf8'), /星忆|星域|星辰|星图/, file)
    }
  }
})
