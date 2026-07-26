import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'

test('chat renders thinking and tool activity above one uninterrupted response body', async () => {
  const [dock, focus, message, activity] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
  ])
  assert.match(dock, /tools=\{state\.tools \|\| \[\]\}/)
  assert.match(focus, /tools: EntityRecord\[\]/)
  assert.match(focus, /activityFeed,\s+tools,\s+thinkingText,/)
  assert.doesNotMatch(message, /streamPreamble|splitAssistantStreamText|has-stream-split/)
  const activityIndex = message.indexOf('<AgentRunActivity')
  const responseIndex = message.indexOf('<MarkdownMessage')
  assert.ok(activityIndex >= 0)
  assert.ok(responseIndex > activityIndex)
  assert.match(activity, /agent-thinking-window/)
  assert.match(activity, /thinkingScrollRef/)
  assert.match(activity, /<MarkdownMessage>\{thinking\}<\/MarkdownMessage>/)
})

test('composer is the sole persistent Agent run status surface', async () => {
  const [focus, styles] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])
  assert.match(focus, /focus-composer-status/)
  assert.match(focus, /compaction\?\.active \? 'compacting' : streaming \? 'running' : 'idle'/)
  assert.doesNotMatch(focus, /focusSession\.agentRunning/)
  assert.match(styles, /\.focus-composer-status\.running/)
  assert.match(styles, /\.focus-composer-status\.compacting/)
})

test('composer send action becomes the only stop control while streaming', async () => {
  const [focus, styles] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])
  assert.doesNotMatch(focus, /className="button danger tiny" onClick=\{onAbort\}/)
  assert.match(focus, /type=\{streaming \? 'button' : 'submit'\}/)
  assert.match(focus, /className=\{`send-button\$\{streaming \? ' stop' : ''\}`\}/)
  assert.match(focus, /onClick=\{streaming \? onAbort : undefined\}/)
  assert.match(focus, /streaming \? \(\s*<Square size=\{16\} fill="currentColor"/)
  assert.match(styles, /\.focus-composer \.send-button\.stop/)
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

test('every settled SSE run reconciles its optimistic message with the durable transcript', async () => {
  const source = await readFile('src/features/chat/ChatPage.tsx', 'utf8')
  const settledRun = source.slice(
    source.indexOf('// Always replace the optimistic SSE bubble'),
    source.indexOf('let completed', source.indexOf('// Always replace the optimistic SSE bubble')),
  )
  assert.match(settledRun, /await loadSessionMessages\(sessionId, \{ force: true \}\)/)
  assert.ok(
    settledRun.indexOf('await loadSessionMessages') < settledRun.indexOf('if ('),
    'durable transcript reconciliation must not depend on Goal or queued-input state',
  )
})

test('assistant text block completion flushes the typewriter and switches to final markdown', async () => {
  const source = await readFile('src/features/chat/ChatPage.tsx', 'utf8')
  const textEndHandler = source.slice(
    source.indexOf("event === 'text_end'"),
    source.indexOf("event === 'thinking_reset'"),
  )
  assert.match(textEndHandler, /responseRenderingStreaming = false/)
  assert.match(textEndHandler, /typewriter\.setTarget\(responseText, data\.updatedAt \|\| eventAt\)/)
  assert.match(textEndHandler, /typewriter\.flush\(\)/)
  assert.match(source, /streaming: responseRenderingStreaming/)
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
  assert.match(
    focus,
    /isLatestAgent &&\s+\(streaming \|\| String\(thinkingText \|\| ''\)\.trim\(\) \|\| tools\.length > 0\)/,
  )
  assert.match(activity, /if \(!streaming && !thinking && !activities\.length\) return null/)
  assert.match(activity, /reasoningCompleted/)
})

test('shared task board uses live/session fallback and consistent effective blockers', async () => {
  const [dock, board] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/TaskBoard.tsx', 'utf8'),
  ])
  assert.match(dock, /resolveSessionTaskList\(sessionState, session\)/)
  assert.match(dock, /isTaskListActive\(taskList, \{ streaming: state\.streaming \}\)/)
  assert.match(dock, /taskList=\{visibleTaskList\}/)
  assert.match(board, /item\.status === 'blocked' \|\| blockedBy\.length > 0/)
  assert.match(board, /view\.status === 'in_progress' && !view\.blocked/)
  assert.doesNotMatch(board, /unblocks:/)
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
