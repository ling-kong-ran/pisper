import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('icon-only composer model control hides the Radix Select trigger content', async () => {
  const [component, styles] = await Promise.all([
    readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(component, /<div\s+className=\{`session-model-select icon-only/)
  assert.doesNotMatch(component, /<label\s+className=\{`session-model-select icon-only/)
  assert.match(
    styles,
    /\.session-model-select\.icon-only \[data-slot='select-trigger'\] \{[^}]*position: absolute;[^}]*inset: 0;[^}]*opacity: 0;/,
  )
  assert.match(
    styles,
    /\.session-model-select\.icon-only \{[^}]*min-width: 38px;[^}]*overflow: hidden;/,
  )
})

test('composer collapses low-frequency controls into an animated tool tray', async () => {
  const [session, tray, styles] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ComposerToolTray.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(session, /className={`composer-tools-trigger/)
  assert.match(session, /<ComposerToolTray[\s\S]*open={toolsOpen}/)
  assert.match(session, /aria-expanded={toolsOpen}/)
  assert.match(tray, /import\('@\/components\/react-bits\/AnimatedList'\)/)
  assert.match(tray, /<Suspense fallback={tray}>/)
  assert.match(tray, /<AnimatedList/)
  assert.match(styles, /\.composer-tool-tray \{[^}]*flex-wrap: wrap;/)
  assert.match(styles, /\.focus-composer-footer \{[^}]*display: flex;/)
})

test('composer exposes a session thinking-level control wired to the shared API', async () => {
  const [controls, session, api, styles] = await Promise.all([
    readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/chat-api.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(controls, /export function SessionThinkingSelect/)
  assert.match(controls, /session-thinking-select/)
  assert.match(session, /SessionThinkingSelect/)
  assert.match(session, /onThinkingLevelChange/)
  assert.match(api, /getThinkingLevel/)
  assert.match(api, /setThinkingLevel/)
  assert.match(api, /thinking-level/)
  assert.match(styles, /\.focus-composer \.session-thinking-select/)
})
