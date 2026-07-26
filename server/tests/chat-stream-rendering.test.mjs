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

test('ephemeral reasoning remains rendered after a textless response completes', async () => {
  const [page, focus, activity] = await Promise.all([
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
  ])
  const doneHandler = page.slice(page.indexOf("event === 'done'"), page.indexOf("event === 'error'"))
  const finalizer = page.slice(page.indexOf('} finally {', page.indexOf("event === 'done'")), page.indexOf('const queuePrompt'))
  assert.match(doneHandler, /thinkingScheduler\.flush\(\)/)
  assert.doesNotMatch(doneHandler, /thinkingText:\s*''/)
  assert.doesNotMatch(finalizer, /thinkingText:\s*''/)
  assert.match(focus, /isLatestAgent && \(streaming \|\| String\(thinkingText \|\| ''\)\.trim\(\)\)/)
  assert.match(activity, /if \(!streaming && !String\(thinkingText \|\| ''\)\.trim\(\)\) return null/)
  assert.match(activity, /reasoningCompleted/)
})

test('bash tool output stays multiline while terminal rendering remains bounded', async () => {
  const [runtime, page, activity, terminal, terminalOutput, styles, packageJson] = await Promise.all([
    readFile('server/runtime/agent-runtime.mjs', 'utf8'),
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/components/ai-elements/terminal.tsx', 'utf8'),
    readFile('src/lib/terminal-output.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('package.json', 'utf8'),
  ])
  assert.match(runtime, /const rawOutput = textFromContent\(event\.partialResult\?\.content\)/)
  assert.match(runtime, /event\.toolName === 'bash' \? \{ output: rawOutput \} : \{\}/)
  assert.match(runtime, /const resultOutput = event\.toolName === 'bash'/)
  assert.match(page, /data\.output !== undefined \? \{ output: data\.output \} : \{\}/)
  assert.match(activity, /import\('@\/components\/ai-elements\/terminal'\)/)
  assert.match(activity, /activity\.name === 'bash'/)
  assert.match(activity, /output=\{String\(activity\.output \|\| ''\)\}/)
  assert.doesNotMatch(terminal, /ansi-to-react/)
  assert.match(terminal, /\{display\.text\}/)
  assert.match(terminal, /TERMINAL_STREAM_PAINT_INTERVAL_MS = 500/)
  assert.match(terminal, /setRenderedOutput\(latestOutputRef\.current\)/)
  assert.match(terminal, /containerRef\.current\.scrollTop = containerRef\.current\.scrollHeight/)
  assert.match(terminalOutput, /MAX_TERMINAL_DISPLAY_CHARS = 4_000/)
  assert.match(styles, /\.agent-run-terminal \{/)
  assert.equal(JSON.parse(packageJson).dependencies['ansi-to-react'], undefined)
})
