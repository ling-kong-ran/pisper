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
  assert.match(activity, /<MarkdownMessage streaming=\{streaming\}>\{thinking\}<\/MarkdownMessage>/)
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

test('completed activity-only messages do not render an empty error bubble', async () => {
  const message = await readFile('src/features/chat/ChatMessage.tsx', 'utf8')
  assert.match(message, /const displayText = fullText \|\| \(!showRunActivity/)
  assert.match(message, /\{displayText && <MarkdownMessage/)
  assert.doesNotMatch(message, /\(fullText \|\| !streaming\)/)
})

test('core chat activity loads synchronously with the message renderer', async () => {
  const message = await readFile('src/features/chat/ChatMessage.tsx', 'utf8')
  assert.match(
    message,
    /import AgentRunActivity, \{ type AgentRunActivityProps \} from '\.\/AgentRunActivity'/,
  )
  assert.doesNotMatch(message, /lazy\(\(\) => .*AgentRunActivity/)
  assert.doesNotMatch(message, /import\('\.\/AgentRunActivity'\)/)
  assert.doesNotMatch(message, /agent-run-activity-placeholder/)
})

test('live snapshots cannot overwrite a locally owned SSE assistant message', async () => {
  const [liveSync, promptCommands] = await Promise.all([
    readFile('src/features/chat/use-live-session-sync.ts', 'utf8'),
    readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  ])
  assert.match(promptCommands, /localStreamSessionsRef\.current\.add\(sessionId\)/)
  assert.match(promptCommands, /localStreamSessionsRef\.current\.delete\(sessionId\)/)
  assert.match(liveSync, /if \(localStreamSessionsRef\.current\.has\(id\)\) return/)
  assert.match(liveSync, /liveSyncInFlightRef\.current\.has\(id\)/)
})

test('every settled SSE run reconciles its optimistic message with the durable transcript', async () => {
  const source = await readFile('src/features/chat/use-prompt-commands.ts', 'utf8')
  const reconcileComment = '// Reconcile every optimistic SSE bubble'
  const settledRun = source.slice(
    source.indexOf(reconcileComment),
    source.indexOf('let completed', source.indexOf(reconcileComment)),
  )
  assert.match(settledRun, /await loadSessionMessages\(sessionId, \{ force: true \}\)/)
  assert.ok(
    settledRun.indexOf('await loadSessionMessages') < settledRun.indexOf('if ('),
    'durable transcript reconciliation must not depend on Goal or queued-input state',
  )
})

test('assistant text block completion flushes the typewriter and settles Markdown streaming', async () => {
  const [dispatcher, promptCommands] = await Promise.all([
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  ])
  const textEndHandler = dispatcher.slice(
    dispatcher.indexOf("event === 'text_end'"),
    dispatcher.indexOf("event === 'thinking_reset'"),
  )
  assert.match(textEndHandler, /state\.responseRenderingStreaming = false/)
  assert.match(
    textEndHandler,
    /typewriter\.setTarget\(state\.responseText, data\.updatedAt \|\| eventAt\)/,
  )
  assert.match(textEndHandler, /typewriter\.flush\(\)/)
  assert.match(promptCommands, /streaming: streamState\.responseRenderingStreaming/)
})

test('ephemeral reasoning remains rendered after a textless response completes', async () => {
  const [dispatcher, promptCommands, transcript, virtualTranscript, activity, sessionState] =
    await Promise.all([
      readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
      readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
      readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
      readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
      readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
      readFile('src/lib/session-state.ts', 'utf8'),
    ])
  const doneHandler = dispatcher.slice(
    dispatcher.indexOf("event === 'done'"),
    dispatcher.indexOf('return { dispatch, state }'),
  )
  const finalizer = promptCommands.slice(
    promptCommands.indexOf('} finally {'),
    promptCommands.indexOf('const queuePrompt'),
  )
  const thinkingResetHandler = dispatcher.slice(
    dispatcher.indexOf("event === 'thinking_reset'"),
    dispatcher.indexOf("event === 'tool_start'"),
  )
  assert.match(doneHandler, /thinkingScheduler\.flush\(\)/)
  assert.doesNotMatch(doneHandler, /thinkingText:\s*''/)
  assert.doesNotMatch(finalizer, /thinkingText:\s*''/)
  assert.match(thinkingResetHandler, /state\.thinkingPrefix = String\(data\.thinkingText \|\| ''\)/)
  assert.match(
    thinkingResetHandler,
    /\[state\.thinkingPrefix, state\.thinkingText\][\s\S]*\.filter\(Boolean\)/,
  )
  assert.match(transcript, /<VirtualMessageTranscript/)
  assert.match(
    virtualTranscript,
    /resolveMessageRunActivity\(message, isLatestAgent, latestRunProps\)/,
  )
  assert.match(sessionState, /String\(activity\.thinkingText \|\| ''\)\.trim\(\)/)
  assert.match(sessionState, /activity\.currentActivity\?\.type === 'agent'/)
  assert.match(activity, /if \(!streaming && !thinking && !activities\.length\) return null/)
  assert.match(activity, /reasoningCompleted/)
})

test('background Agent completion uses code-level UI state without prompt or custom-context injection', async () => {
  const [runtime, dispatcher, virtualTranscript, sessionState] = await Promise.all([
    readFile('runtime/runtime/agent-runtime.mjs', 'utf8'),
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('src/features/chat/VirtualMessageTranscript.tsx', 'utf8'),
    readFile('src/lib/session-state.ts', 'utf8'),
  ])
  assert.doesNotMatch(runtime, /sendCustomMessage/)
  assert.doesNotMatch(runtime, /pisper_agent_mailbox_results/)
  assert.match(runtime, /live\.currentActivity = backgroundActivities\.at\(-1\) \|\| null/)
  assert.match(dispatcher, /data\.currentActivity\?\.type === 'agent'/)
  assert.match(
    virtualTranscript,
    /resolveMessageRunActivity\(message, isLatestAgent, latestRunProps\)/,
  )
  assert.match(sessionState, /activity\.currentActivity\?\.type === 'agent'/)
})

test('stale streaming queue errors settle the old stream and resend as a new turn', async () => {
  const source = await readFile('src/features/chat/use-prompt-commands.ts', 'utf8')
  const queueHandler = source.slice(
    source.indexOf('const queuePrompt'),
    source.indexOf('const abort'),
  )
  assert.match(queueHandler, /isEndedSessionQueueError\(error\)/)
  assert.match(queueHandler, /if \(activeStream\) await activeStream\.promise/)
  assert.match(queueHandler, /await loadSessionMessages\(sessionId, \{ force: true \}\)/)
  assert.match(queueHandler, /void sendPrompt\(message, sessionId\)/)
  assert.doesNotMatch(
    queueHandler,
    /notify\(chatErrorMessage\(error\), 'error'\)[\s\S]*isEndedSessionQueueError/,
  )
})

test('shared plan board uses live/session fallback and consistent effective blockers', async () => {
  const [dock, board] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/PlanBoard.tsx', 'utf8'),
  ])
  assert.match(dock, /resolveSessionPlan\(sessionState, session\)/)
  assert.match(dock, /isPlanActive\(plan, \{ streaming: state\.streaming \}\)/)
  assert.match(dock, /plan=\{visiblePlan\}/)
  assert.match(board, /item\.status === 'blocked' \|\| blockedBy\.length > 0/)
  assert.match(board, /view\.status === 'in_progress' && !view\.blocked/)
  assert.doesNotMatch(board, /unblocks:/)
})

test('bash tool output stays multiline while terminal rendering remains bounded', async () => {
  const [runtime, dispatcher, activity, terminal, terminalOutput, styles, packageJson] =
    await Promise.all([
      readFile('runtime/runtime/agent-runtime.mjs', 'utf8'),
      readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
      readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
      readFile('src/components/ai-elements/terminal.tsx', 'utf8'),
      readFile('src/lib/terminal-output.ts', 'utf8'),
      readFile('src/index.css', 'utf8'),
      readFile('package.json', 'utf8'),
    ])
  assert.match(runtime, /const rawOutput = textFromContent\(event\.partialResult\?\.content\)/)
  assert.match(runtime, /event\.toolName === 'bash' \? \{ output: rawOutput \} : \{\}/)
  assert.match(runtime, /const resultOutput = event\.toolName === 'bash'/)
  assert.match(dispatcher, /data\.output !== undefined \? \{ output: data\.output \} : \{\}/)
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

test('chat keeps hidden panels and continuous canvas work out of the renderer', async () => {
  const [dock, dispatcher, ascii] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('src/components/react-bits/AsciiText.tsx', 'utf8'),
  ])
  assert.match(dock, /api\.onDidVisibilityChange/)
  assert.match(dock, /if \(!visible\) return null/)
  assert.match(dock, /if \(visible && sessionId\)/)
  assert.match(dispatcher, /tools: pushCurrentActivity\(current\.tools, activity\)/)
  assert.match(ascii, /new ResizeObserver\(draw\)/)
  assert.doesNotMatch(ascii, /requestAnimationFrame|setInterval/)
})
