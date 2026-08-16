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

test('composer expands low-frequency controls horizontally with React Bits AnimatedContent', async () => {
  const [session, tray, animatedContent, styles] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ComposerToolTray.tsx', 'utf8'),
    readFile('src/components/react-bits/AnimatedContent.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(session, /focus-composer[^"\n]*\[&_textarea\]:\[outline:0\]!/)
  assert.match(session, /className={`composer-tools-trigger/)
  assert.match(session, /<ComposerToolTray[\s\S]*open={toolsOpen}/)
  assert.match(session, /aria-expanded={toolsOpen}/)
  assert.match(tray, /import\('@\/components\/react-bits\/AnimatedContent'\)/)
  assert.match(tray, /<Suspense fallback={null}>/)
  assert.match(tray, /direction="horizontal"/)
  assert.match(animatedContent, /width: direction === 'horizontal' \? 0 : 'auto'/)
  assert.match(animatedContent, /useReducedMotion/)
  assert.doesNotMatch(animatedContent, /<AnimatePresence initial={false}>/)
  assert.match(tray, /composer-tool-tray[^"\n]*flex/)
  assert.match(session, /command-palette-trigger[^"\n]*composer-tool-tray_&\]:w-\[38px\]/)
  assert.match(session, /command-palette-trigger[^"\n]*clip-path:inset\(50%\)/)
  assert.doesNotMatch(session, /<span>{t\('chat:focusSession.commands'\)}<\/span>/)
  assert.doesNotMatch(
    styles,
    /\.composer-tool-tray \.command-palette-trigger \{[^}]*min-width: 112px;/,
  )
  assert.doesNotMatch(styles, /\.composer-tool-tray \{[^}]*flex-wrap: wrap;/)
  assert.match(
    session,
    /<ExecutionModeSelect[\s\S]*<div className="focus-composer-quick-actions[^"\n]*">/,
  )
  assert.match(session, /toolsOpen \? <ChevronsLeft[^:]+: <ChevronsRight/)
  assert.match(session, /focus-composer-quick-actions[^"\n]*tools-open_&\]:flex-1/)
  assert.doesNotMatch(session, /focus-composer-secondary[^"\n]*tools-open_&\]:hidden/)
  assert.match(session, /composer-workspace-status[\s\S]*<SessionUsageMetrics/)
  assert.doesNotMatch(tray, /composer-workspace/)
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
