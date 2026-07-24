import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('chat rendering keeps tool state connected to streaming message segmentation', async () => {
  const [dock, focus, message] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
  ])
  assert.match(dock, /tools=\{state\.tools \|\| \[\]\}/)
  assert.match(focus, /tools: EntityRecord\[\]/)
  assert.match(focus, /activityFeed,\s+tools,\s+thinkingText,/)
  assert.match(message, /const hasTools = Boolean\(runProps\?\.tools\?\.length\)/)
})

test('chat activity lazy loading reserves layout space instead of flashing an empty fallback', async () => {
  const [message, styles] = await Promise.all([
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])
  assert.doesNotMatch(message, /<Suspense fallback=\{null\}>/)
  assert.match(message, /agent-run-activity-placeholder/)
  assert.match(styles, /\.agent-run-activity-placeholder \{ min-height: 42px;/)
})

test('live snapshots cannot overwrite a locally owned SSE assistant message', async () => {
  const source = await readFile('src/features/chat/ChatPage.tsx', 'utf8')
  assert.match(source, /localStreamSessionsRef\.current\.add\(sessionId\)/)
  assert.match(source, /localStreamSessionsRef\.current\.delete\(sessionId\)/)
  assert.match(source, /if \(localStreamSessionsRef\.current\.has\(id\)\) return/)
  assert.match(source, /liveSyncInFlightRef\.current\.has\(id\)/)
})
