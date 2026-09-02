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

test('composer expands low-frequency controls vertically with React Bits AnimatedContent', async () => {
  const [session, tray, animatedContent, animatedList, styles, menuOffset] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ComposerToolTray.tsx', 'utf8'),
    readFile('src/components/react-bits/AnimatedContent.tsx', 'utf8'),
    readFile('src/components/react-bits/AnimatedList.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/features/chat/use-viewport-menu-offset.ts', 'utf8'),
  ])

  assert.match(session, /focus-composer[^"\n]*\[&_textarea\]:\[outline:0\]!/)
  assert.match(session, /className={`composer-tools-trigger/)
  assert.match(session, /<ComposerToolTray[\s\S]*open={toolsOpen}/)
  assert.match(session, /aria-expanded={toolsOpen}/)
  assert.match(session, /document\.addEventListener\('pointerdown', closeOnPointerDown\)/)
  // 外部点击关闭需同时排除触发按钮、工具托盘与锚定弹出菜单三个区域。
  assert.match(
    session,
    /target\.closest\('\.composer-tools-trigger, \.composer-tool-tray-shell, \.anchored-popup-menu'\)/,
  )
  assert.match(tray, /import\('@\/components\/react-bits\/AnimatedContent'\)/)
  assert.match(tray, /<Suspense fallback={null}>/)
  assert.match(tray, /direction="vertical"/)
  assert.match(tray, /className="absolute bottom-\[calc\(100%\+12px\)\]/)
  assert.match(tray, /distance=\{16\}/)
  assert.match(tray, /reveal/)
  assert.match(tray, /spring/)
  assert.match(tray, /allowOverflow/)
  assert.match(tray, /delegateClicks/)
  assert.match(tray, /variant="burst"/)
  assert.match(tray, /composer-energy-spin/)
  assert.match(animatedContent, /reveal\?: boolean/)
  assert.match(animatedContent, /allowOverflow\?: boolean/)
  assert.match(animatedContent, /overflow: allowOverflow \|\| settled \? 'visible' : 'hidden'/)
  assert.match(animatedContent, /type: 'spring'/)
  assert.match(animatedContent, /useReducedMotion/)
  assert.match(animatedList, /function flattenedChildren/)
  assert.match(animatedList, /child\.type === Fragment/)
  assert.match(animatedList, /const childKey = `\$\{parentKey\}/)
  assert.match(animatedList, /delegateClicks/)
  assert.match(animatedList, /querySelector<HTMLElement>/)
  assert.match(animatedList, /delay: reduceMotion \? 0 : 0\.045 \+ index \* 0\.038/)
  assert.match(tray, /composer-tool-tray-shell[^"\n]*overflow-visible/)
  assert.match(tray, /composer-tool-tray[^"\n]*flex-wrap/)
  assert.match(tray, /!size-11 !w-11/)
  assert.doesNotMatch(tray, /\[&:hover>\*\]/)
  assert.doesNotMatch(tray, /if \(mobile\) return tray|overflow-x-auto/)
  assert.match(session, /<SessionModelSelect[\s\S]*<ExecutionModeSelect/)
  assert.match(session, /<ComposerToolTray[\s\S]*composer-tool-tray/)
  assert.ok(session.indexOf('<ComposerCommandMenu') < session.indexOf('<textarea'))
  const leadingTools = session.slice(
    session.indexOf('const composerLeadingTools'),
    session.indexOf('// 空会话头部'),
  )
  assert.doesNotMatch(leadingTools, /ComposerCommandMenu/)
  assert.match(menuOffset, /--menu-y-offset/)
  assert.match(menuOffset, /\(auto\|hidden\|scroll\|clip\)/)
  assert.match(menuOffset, /menu\.style\.maxHeight/)
  assert.match(session, /toolsOpen \? <X size=\{17\} \/> : <Plus size=\{18\} \/>/)
  assert.match(session, /focus-composer-footer[^"\n]*flex/)
  assert.doesNotMatch(session, /toolsOpen \? 'tools-open grid-rows|toolsOpen \? 'row-start-2'/)
  assert.match(session, /<ContextUsageIndicator[\s\S]*?compact[\s\S]*?\/>/)
  assert.match(session, /command-palette-trigger[^"\n]*clip-path:inset\(50%\)/)
  assert.doesNotMatch(session, /<span>{t\('chat:focusSession.commands'\)}<\/span>/)
  assert.doesNotMatch(
    styles,
    /\.composer-tool-tray \.command-palette-trigger \{[^}]*min-width: 112px;/,
  )
  assert.doesNotMatch(session, /focus-composer-secondary[^"\n]*tools-open_&\]:hidden/)
  assert.match(session, /composer-workspace-status[\s\S]*<SessionUsageMetrics/)
  assert.doesNotMatch(tray, /composer-workspace/)
})

test('composer plain Enter submits, Shift+Enter inserts a newline, and IME composition never submits', async () => {
  const session = await readFile('src/features/chat/FocusSession.tsx', 'utf8')

  // 发送行为锁定：Enter 直接发送是产品约定，不允许改为 Ctrl/⌘+Enter。
  assert.match(session, /event\.key === 'Enter' &&\s*!event\.shiftKey &&\s*!composing/)
  assert.doesNotMatch(session, /submitsWithShortcut|metaKey \|\| event\.ctrlKey/)
  assert.match(session, /event\.currentTarget\.form\?\.requestSubmit\(\)/)
  assert.match(session, /enterKeyHint=\{mobileLayout \? 'send' : 'enter'\}/)
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
