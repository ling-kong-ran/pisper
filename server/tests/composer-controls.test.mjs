import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('icon-only composer model control hides the Radix Select trigger content', async () => {
  const [component, styles] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])

  assert.match(component, /<div\s+className=\{`session-model-select icon-only/)
  assert.doesNotMatch(component, /<label\s+className=\{`session-model-select icon-only/)
  assert.match(
    styles,
    /\.session-model-select\.icon-only \[data-slot='select-trigger'\] \{[^}]*position: absolute;[^}]*inset: 0;[^}]*opacity: 0;/,
  )
  assert.match(styles, /\.session-model-select\.icon-only \{[^}]*min-width: 38px;[^}]*overflow: hidden;/)
})
