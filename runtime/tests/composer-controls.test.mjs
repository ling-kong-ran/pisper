import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('icon-only composer model control hides the Radix Select trigger content', async () => {
  const component = await readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8')

  assert.match(component, /<div\s+className=\{`session-model-select[^`]*icon-only/)
  assert.doesNotMatch(component, /<label\s+className=\{`session-model-select icon-only/)
  assert.match(
    component,
    /session-model-select&_[^`]*select-trigger[^`]*absolute[^`]*inset-0[^`]*opacity-0/,
  )
  assert.match(component, /session-model-select&\]:min-w-\[38px\][^`]*overflow-hidden/)
})

test('composer keeps shortcuts inline and overflows them by measured panel width', async () => {
  const [session, tray, layout, capacity, settings, store] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ComposerToolTray.tsx', 'utf8'),
    readFile('src/features/chat/composer-toolbar-layout.ts', 'utf8'),
    readFile('src/features/chat/use-composer-toolbar-capacity.ts', 'utf8'),
    readFile('src/features/chat/ComposerToolbarSettings.tsx', 'utf8'),
    readFile('src/stores/composer-toolbar-store.ts', 'utf8'),
  ])

  assert.match(session, /focus-composer[^"\n]*\[&_textarea\]:\[outline:0\]!/)
  assert.match(session, /className={`composer-tools-trigger/)
  assert.match(session, /aria-expanded={toolsOpen}/)
  assert.match(session, /ref={toolbarRef}/)
  assert.match(session, /toolbarAllocation\.inline\.map\(renderComposerTool\)/)
  assert.match(session, /toolbarAllocation\.overflow\.map\(renderComposerTool\)/)
  assert.match(session, /<ComposerToolbarSettings labels={composerToolLabels}/)
  assert.match(session, /<ContextUsageIndicator[\s\S]*?compact[\s\S]*?\/>/)
  assert.match(session, /toolsOpen \? <X size=\{17\} \/> : <Plus size=\{18\} \/>/)
  assert.match(session, /document\.addEventListener\('pointerdown', closeOnPointerDown\)/)
  // portal 内的托盘、设置 Dialog 和工具派生浮层都不能触发外部点击关闭。
  assert.match(session, /target\.closest\(TRAY_FLOATING_SELECTOR\)/)
  assert.match(session, /'\.composer-tool-tray-shell'/)
  assert.match(session, /"\[data-slot='dialog-content'\]"/)
  assert.match(session, /"\[data-slot='dialog-overlay'\]"/)

  assert.match(layout, /export const COMPOSER_TOOL_IDS = \[/)
  assert.match(layout, /normalizeComposerToolbarLayout/)
  assert.match(layout, /automaticallyOverflowed/)
  assert.match(layout, /preferredInline\.slice\(0, capacity\)/)
  assert.match(capacity, /new ResizeObserver\(update\)/)
  assert.match(capacity, /getBoundingClientRect\(\)\.width/)
  assert.match(store, /name: 'pisper-composer-toolbar'/)
  assert.match(store, /normalizeComposerToolbarLayout/)

  assert.match(tray, /<AnchoredPopupMenu/)
  assert.match(tray, /placement="top"/)
  assert.match(tray, /composer-tool-tray[^"\n]*flex-wrap/)
  assert.doesNotMatch(tray, /AnimatedContent|AnimatedList|composer-energy-spin/)
  assert.match(settings, /setToolLocation/)
  assert.match(settings, /moveTool/)
  assert.match(settings, /resetLayout/)
  assert.ok(session.indexOf('<ComposerCommandMenu') < session.indexOf('<textarea'))
  assert.doesNotMatch(session, /focus-composer-secondary[^"\n]*tools-open_&\]:hidden/)
  assert.match(session, /composer-workspace-status[\s\S]*<SessionUsageMetrics/)
})

test('composer plain Enter submits, Shift+Enter inserts a newline, and IME composition never submits', async () => {
  const session = await readFile('src/features/chat/FocusSession.tsx', 'utf8')

  // 默认仍为 Enter；自定义绑定统一经精确匹配，不能绕过组词与换行保护。
  const { DEFAULT_SHORTCUTS, matchesShortcut } = await import('../../shared/shortcuts.mjs')
  assert.equal(DEFAULT_SHORTCUTS.sendMessage, 'Enter')
  assert.equal(matchesShortcut({ code: 'Enter' }, DEFAULT_SHORTCUTS.sendMessage), true)
  assert.equal(
    matchesShortcut({ code: 'Enter', shiftKey: true }, DEFAULT_SHORTCUTS.sendMessage),
    false,
  )
  assert.equal(
    matchesShortcut({ code: 'Enter', isComposing: true }, DEFAULT_SHORTCUTS.sendMessage),
    false,
  )
  assert.equal(matchesShortcut({ code: 'Enter' }, 'Mod+Enter'), false)
  assert.equal(matchesShortcut({ code: 'Enter', ctrlKey: true }, 'Mod+Enter'), true)
  assert.equal(
    matchesShortcut({ code: 'Enter', ctrlKey: true, shiftKey: true }, 'Mod+Enter'),
    false,
  )
  assert.equal(matchesShortcut({ code: 'Enter', ctrlKey: true, keyCode: 229 }, 'Mod+Enter'), false)
  assert.match(
    session,
    /!composing && matchesShortcut\(event\.nativeEvent, shortcuts\.sendMessage\)/,
  )
  assert.match(session, /useShortcutStore\(\(state\) => state\.bindings\)/)
  assert.match(session, /event\.currentTarget\.form\?\.requestSubmit\(\)/)
  assert.match(
    session,
    /enterKeyHint=\{mobileLayout && shortcuts\.sendMessage === 'Enter' \? 'send' : 'enter'\}/,
  )
  // IME 组词保护双保险：Chromium 靠 isComposing；Mac WebKit 的确认 Enter
  // 在 compositionend 之后派发，靠自行跟踪的 imeComposingRef 延迟复位覆盖。
  assert.match(session, /event\.nativeEvent\.isComposing \|\| imeComposingRef\.current/)
  assert.match(session, /onCompositionStart/)
  assert.match(session, /onCompositionEnd/)
})

test('composer exposes a session thinking-level control wired to the shared API', async () => {
  const [controls, session, api] = await Promise.all([
    readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
  ])

  assert.match(controls, /export function SessionThinkingSelect/)
  assert.match(controls, /session-thinking-select/)
  assert.match(session, /SessionThinkingSelect/)
  assert.match(session, /onThinkingLevelChange/)
  assert.match(api, /getThinkingLevel/)
  assert.match(api, /setThinkingLevel/)
  assert.match(api, /thinking-level/)
  assert.match(controls, /session-thinking-select[^`\n]*\.focus-composer_&\]:flex-none/)
})
