import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { TooltipProvider } from '../../src/components/ui/tooltip.tsx'
import { FocusChatMessage } from '../../src/features/chat/ChatMessage.tsx'
import {
  ComposerSendButton,
  ComposerTray,
} from '../../src/features/chat/focus-session-composer-bits.tsx'

test('foreground resume refreshes cached sessions and invalidates stale SSE ownership', async () => {
  const [page, liveSync, prompt] = await Promise.all([
    readFile('src/features/chat/ChatPage.tsx', 'utf8'),
    readFile('src/features/chat/use-live-session-sync.ts', 'utf8'),
    readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
  ])
  assert.match(page, /document\.addEventListener\('visibilitychange', recover\)/)
  assert.match(page, /window\.addEventListener\('pageshow', recover\)/)
  assert.match(page, /window\.addEventListener\('online', recover\)/)
  assert.match(page, /await refreshSessions\(undefined, \{ preserveExistingOnEmpty: true \}\)/)
  assert.match(page, /syncLiveSession\(id, \{ force: true \}\)/)
  assert.match(liveSync, /async \(id: string, \{ force = false \}/)
  assert.match(liveSync, /if \(force && localStreamSessionsRef\.current\.has\(id\)\)/)
  assert.match(liveSync, /streamGenerationRef\.current\.set\(id, generation \+ 1\)/)
  assert.match(prompt, /if \(!ownsStream\(\)\) return false/)
  assert.match(
    prompt,
    /if \(currentStream\) \{[\s\S]*streamGenerationRef\.current\.delete\(sessionId\)/,
  )
})

test('chat renders thinking and tool activity above one uninterrupted response body', async () => {
  const [dock, focus, focusProps, message, activity] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/focus-session-props.ts', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
  ])
  // 空数组兑底用共享常量，避免每次渲染新建数组击穿 FocusSession 的 memo。
  assert.match(dock, /const EMPTY_LIST: never\[\] = \[\]/)
  assert.match(dock, /tools=\{state\.tools \|\| EMPTY_LIST\}/)
  // 此处只保护渲染接线；旧 OR 会让陈旧摘要覆盖已完成状态，优先级由 session-streaming-state 行为测试覆盖。
  assert.match(dock, /resolveSessionStreaming\(state, session\)/)
  assert.match(focusProps, /tools: EntityRecord\[\]/)
  assert.match(focus, /activityFeed,\s+tools,\s+thinkingText,/)
  assert.doesNotMatch(message, /streamPreamble|splitAssistantStreamText|has-stream-split/)
  const activityIndex = message.indexOf('<AgentRunActivity')
  const responseIndex = message.indexOf('<MarkdownMessage')
  assert.ok(activityIndex >= 0)
  assert.ok(responseIndex > activityIndex)
  assert.match(activity, /agent-thinking-window/)
  assert.match(activity, /thinkingScrollRef/)
  assert.match(activity, /<MarkdownMessage streaming=\{streaming\}>\{thinking\}<\/MarkdownMessage>/)
  assert.match(activity, /agent-thinking-window[^`]*\$\{streaming \? 'running' : 'completed'\}/)
  assert.match(activity, /agentRunActivity\.reasoningInProgress/)
  assert.match(activity, /className="agent-thinking-dots[^"\n]*"/)
  assert.doesNotMatch(activity, /<details[^>]*agent-team-panel[^>]*\sopen(?:=|\s|>)/)
  assert.match(activity, /className="agent-run-history[^"\n]*"/)
})

test('streaming chat keeps its text lifecycle without decorative SSE motion', async () => {
  const [activity, transcript, styles, markdown] = await Promise.all([
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/index.css', 'utf8'),
    readFile('src/components/MarkdownMessage.tsx', 'utf8'),
  ])

  assert.match(markdown, /aria-busy=\{streaming \|\| undefined\}/)
  assert.match(markdown, /isAnimating=\{streaming\}/)
  assert.doesNotMatch(markdown, /sse-|updateStreamingLineSpectrum|getClientRects/)
  assert.doesNotMatch(activity, /data-pisper-streaming/)
  assert.doesNotMatch(transcript, /data-pisper-streaming/)
  assert.doesNotMatch(styles, /sse-/)
})

test('SSE activity isolates one total-duration clock from the streamed activity tree', async () => {
  const activity = await readFile('src/features/chat/AgentRunActivity.tsx', 'utf8')
  const durationLabel = activity.slice(
    activity.indexOf('function RunDurationLabel'),
    activity.indexOf('function cleanInline'),
  )
  const activityCard = activity.slice(
    activity.indexOf('const ActivityCard = memo'),
    activity.indexOf('function AgentRunActivity'),
  )
  const activityPanel = activity.slice(activity.indexOf('function AgentRunActivity'))

  assert.match(durationLabel, /const now = useRunActivityClock\(streaming\)/)
  assert.match(durationLabel, /runDurationMs\(startedAt, finishedAt, now\)/)
  assert.doesNotMatch(activityCard, /agent-run-duration|activityDurationMs|useRunActivityClock/)
  assert.doesNotMatch(activityPanel, /const now = useRunActivityClock/)
  assert.doesNotMatch(activityPanel, /showOverview|agent-run-overview|ShinyText/)
  assert.match(activityPanel, /<RunDurationLabel/)
  assert.match(activity, /export function runActivityStatusLabel/)
})

test('tool activity uses a polished scroll viewport without truncating records', async () => {
  const activity = await readFile('src/features/chat/AgentRunActivity.tsx', 'utf8')
  assert.match(activity, /ref=\{liveFeedRef\}[\s\S]*agent-run-feed[^"\n]*live/)
  assert.match(activity, /tabIndex=\{activities\.length > 3 \? 0 : undefined\}/)
  assert.match(activity, /if \(!streaming \|\| !activities\.length\) return undefined/)
  assert.match(activity, /node\.scrollTop = node\.scrollHeight/)
  // 工具日志是一张卡片内的细线分隔行：不留行间距，只用上边线分隔。
  assert.match(
    activity,
    /agent-run-feed[^"\n]*max-h-\[184px\][^"\n]*overflow-y-auto[^"\n]*\[&>\*\+\*\]:border-t/,
  )
  assert.match(activity, /agent-run-command-output[^"\n]*max-h-\[320px\]/)
  // 只有运行中的日志高亮最新一行，已完成的日志不再给最后一行着色。
  assert.match(activity, /latest \? 'current[^']*\.live[^']*bg-surface-subtle/)
  assert.match(activity, /activityPlan\?\.items\?\.length[\s\S]*planCleared/)
})

test('composer tray is the sole persistent Agent run status surface', () => {
  // 规格 v2 第 4.3 节：输入框上方的托盘只在运行中、压缩上下文或有排队时出现；
  // 运行行用中性的运行弧与“运行中”，不染成功绿、不用强调色（--star 只留给发送键），也不用琥珀。
  const render = (props) => renderToStaticMarkup(React.createElement(ComposerTray, props))
  assert.equal(render({ streaming: false }), '')
  const running = render({ streaming: true, onStop() {} })
  assert.match(running, /data-composer-tray/)
  assert.match(running, /data-run-bar="running"/)
  assert.match(running, /data-status-glyph="run"/)
  assert.doesNotMatch(running, /star|success|warning|st-attn/)
  const compacting = render({ streaming: false, compaction: { active: true } })
  assert.match(compacting, /data-run-bar="compacting"/)
  assert.doesNotMatch(compacting, /warning|st-attn/)
})

test('all chats expose their working directory in the title header, not in the composer or welcome screen', async () => {
  const [focus, transcript, chinese, english] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/locales/zh-CN/chat.json', 'utf8'),
    readFile('src/locales/en-US/chat.json', 'utf8'),
  ])
  assert.match(focus, /cwd=\{cwd\}/)
  assert.match(focus, /onWorkspace=\{onWorkspace\}/)
  assert.match(focus, /header-workspace/)
  assert.match(focus, /workspaceName\(cwd, language\)/)
  assert.match(focus, /aria-label=\{t\('chat:focusSession\.changeWorkingDirectoryWorkspace'/)
  assert.match(focus, /onClick=\{onWorkspace\}/)
  assert.match(focus, /const headerBlock[\s\S]*\{workspaceBlock\}[\s\S]*onClick=\{onRename\}/)
  assert.doesNotMatch(
    focus.slice(focus.indexOf('const composerBlock')),
    /kind="workspace"|workspaceBlock/,
  )
  assert.doesNotMatch(transcript, /welcome-workspace/)
  // 目录按钮的无障碍名称两种语言都有（原“工作目录”键没有任何调用方，随 A2 清理未用文案删除）。
  assert.match(chinese, /"focusSession\.changeWorkingDirectoryWorkspace": /)
  assert.match(english, /"focusSession\.changeWorkingDirectoryWorkspace": /)
})

test('conversation layout keeps a compact title header without a persistent avatar card', async () => {
  const [focus, message, transcript, pageHeader, shell, phoneHeader] = await Promise.all([
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/ChatMessage.tsx', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/SessionPageHeader.tsx', 'utf8'),
    readFile('src/app/SessionShell.tsx', 'utf8'),
    readFile('src/features/chat/SessionPhoneHeader.tsx', 'utf8'),
  ])
  assert.match(focus, /hasConversation \? 'has-conversation' : 'is-empty'/)
  // 旧的任务顶栏只留给非路由的旧入口（第 6 期删除）。
  assert.match(
    focus,
    /<TaskWorkspaceHeader[\s\S]*session\.name \|\| t\('navigation:pageHeader\.newChat'\)/,
  )
  // 第 4 期手机会话页（规格 7.3）：56px 手机顶栏取代旧任务顶栏，标题同样只出现一次、点按改名、没有头像卡片。
  assert.match(focus, /phonePage \? \(\s*<SessionPhoneHeader\b/)
  assert.match(
    phoneHeader,
    /const title = session\.name \|\| t\('navigation:pageHeader\.newChat'\)/,
  )
  assert.equal(phoneHeader.match(/data-session-title/g)?.length, 1)
  assert.match(
    phoneHeader,
    /onClick=\{actions\.onRename\}[\s\S]{0,40}<span className="truncate" data-session-title>/,
  )
  assert.doesNotMatch(phoneHeader, /Avatar|BrandLogo/)
  // 桌面会话顶栏（规格 v2 第 4.1 节，52px）：标题只出现这一次（15px），是标题 ▾ 菜单的触发键，
  // 菜单第一项就是重命名（原来“点标题改名”的等价入口）；没有头像卡片，外壳本身不再重复标题。
  assert.match(focus, /const headerBlock = pageHeaderBlock \|\| \(/)
  assert.match(pageHeader, /const title = session\.name \|\| t\('navigation:pageHeader\.newChat'\)/)
  assert.match(
    pageHeader,
    /<DropdownMenuTrigger asChild>[\s\S]{0,600}text-\[15px\] leading-6[\s\S]{0,800}data-session-title[\s\S]{0,400}<DropdownMenuContent[^>]*>\s*\{menu\.items\}/,
  )
  const headerMenu = await readFile('src/features/chat/session-header-menu.tsx', 'utf8')
  assert.match(
    headerMenu,
    /const items = \(\s*<>\s*<DropdownMenuItem onSelect=\{actions\.onRename\}>/,
  )
  assert.equal(pageHeader.match(/data-session-title/g)?.length, 1)
  assert.doesNotMatch(pageHeader, /Avatar|BrandLogo/)
  assert.doesNotMatch(shell, /session\.name|data-session-title|pageHeader\.newChat/)
  assert.doesNotMatch(focus, /sessionActionsMenu|<SessionActionsMenu/)
  assert.match(message, /<BrandLogo size=\{22\} \/>/)
  assert.doesNotMatch(message, /AgentStatusAvatar/)
  // 模板可调宽度/间距；保留默认 1040px / 32px，非法配置由布局契约测试拒绝。
  assert.match(message, /max-w-\[var\(--chat-content-width,1040px\)\]/)
  assert.match(message, /mb-\[var\(--chat-message-gap,32px\)\]/)
  assert.match(message, /message\.role === 'agent'\s*\? 'items-stretch'\s*:\s*["']items-end/)
  assert.match(message, /message-content[\s\S]*message\.role === 'agent'[\s\S]*'w-full'/)
  assert.match(message, /agent-message-mark/)
  assert.match(message, /data-state=\{agentState\}/)
  assert.match(transcript, /<WorkbenchGreeting \/>/)
  assert.doesNotMatch(transcript, /WelcomeEffects|WelcomeFallback/)
})

test('composer actions distinguish sending, steering, preparing and blocked states', () => {
  const render = (props) =>
    renderToStaticMarkup(
      React.createElement(ComposerSendButton, {
        queueing: false,
        disabled: false,
        ...props,
      }),
    )
  assert.match(render({}), /type="submit"/)
  assert.doesNotMatch(render({}), / disabled=""/)
  // 发送键从不兼任停止：运行中没有可排队的草稿时只是不可用，停止由运行条上的停止键负责。
  for (const props of [{ streaming: true }, { streaming: true, hasDraft: true, canQueue: false }]) {
    const html = render(props)
    assert.match(html, /type="submit"/)
    assert.match(html, / disabled=""/)
    assert.doesNotMatch(html, /data-action/)
  }
  const steering = render({ streaming: true, hasDraft: true, canQueue: true })
  assert.match(steering, /type="submit"/)
  assert.doesNotMatch(steering, / disabled=""/)
  for (const state of ['queueing', 'submitting', 'stopping', 'preparing']) {
    const html = render({ [state]: true })
    assert.match(html, /aria-busy="true"/)
    assert.match(html, / disabled=""/)
  }
  const blocked = render({ blockedReason: 'Fixture unavailable' })
  assert.match(blocked, /aria-disabled="true"/)
  assert.match(blocked, /aria-label="[^"]*Fixture unavailable"/)
  assert.doesNotMatch(blocked, / disabled=""/)
  // 激活、IME及异步状态切换由 smoke-replica-chat 的真实组件夹具覆盖。
})

test('the tray owns the only stop control, including while compacting context', () => {
  const render = (props) =>
    renderToStaticMarkup(React.createElement(ComposerTray, { onStop() {}, ...props }))
  for (const compaction of [null, { active: true }]) {
    const html = render({ streaming: true, compaction })
    assert.equal(html.match(/<button/g)?.length, 1)
    assert.match(html, /data-run-stop/)
  }
  // 只在压缩（没有运行）时没有停止键：压缩不能被“停止”打断。
  assert.doesNotMatch(render({ streaming: false, compaction: { active: true } }), /<button/)
  assert.match(render({ streaming: true, stopping: true }), /aria-busy="true"[^>]* disabled=""/)
})

test('image previews portal above session-level controls', async () => {
  const message = await readFile('src/features/chat/ChatMessage.tsx', 'utf8')
  assert.match(message, /import \{ createPortal \} from 'react-dom'/)
  assert.match(message, /createPortal\([\s\S]*<ImageLightbox[\s\S]*document\.body/)
  assert.match(message, /<Button\s+asChild\s+size="lg"/)
  assert.match(message, /bg-\[var\(--lightbox-action-bg\)\]/)
  assert.match(message, /text-\[var\(--lightbox-action-text\)\]/)
  assert.doesNotMatch(message, /className="(?:button|icon-button)/)
  assert.match(message, /image-lightbox[^"\n]*fixed[^"\n]*z-\[100\]/)
  assert.doesNotMatch(message, /image-lightbox-toolbar[^"\n]*(?:button|icon-button)/)
})

test('empty completed replies stay empty and request diagnostics never replace the reply body', () => {
  const render = (message) =>
    renderToStaticMarkup(
      React.createElement(
        TooltipProvider,
        null,
        React.createElement(FocusChatMessage, {
          sessionId: 'test',
          message: { id: 'reply', role: 'agent', ...message },
          agentState: 'idle',
          showRunActivity: false,
        }),
      ),
    )
  const rawError = '503 upstream unavailable: <html>provider response</html>'
  const empty = render({ text: '' })
  assert.doesNotMatch(empty, /markdown-body|aria-expanded/)
  for (const text of ['', rawError, 'Partial answer remains readable']) {
    const html = render({ text, error: rawError })
    assert.match(html, /aria-expanded="false"/)
    assert.doesNotMatch(html, /upstream unavailable|provider response|has-error/)
    assert.equal(html.includes('Partial answer remains readable'), text.startsWith('Partial'))
    assert.equal(html.includes('markdown-body'), text.startsWith('Partial'))
  }
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
  assert.match(liveSync, /if \(!force && localStreamSessionsRef\.current\.has\(id\)\) return/)
  assert.match(liveSync, /liveSyncInFlightRef\.current\.has\(id\)/)
})

test('settled SSE runs reconcile metadata from the done frame without a transcript reload', async () => {
  const [source, dispatch, runtime] = await Promise.all([
    readFile('src/features/chat/use-prompt-commands.ts', 'utf8'),
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('runtime/runtime/agent-runtime.mjs', 'utf8'),
  ])
  // done 帧直接携带回合边界条目，前端就地补全消息元数据（资产随 assets 字段）。
  assert.match(runtime, /emit\('done', \{[\s\S]*turnBoundaryEntryId/)
  assert.match(dispatch, /data\.turnBoundaryEntryId/)
  // 排空打字机后直接把本地文本定格；成功路径不再整页强制重拉历史，
  // 避免最后一行因换成服务端 ID 而重挂载。
  const drainIndex = source.indexOf('await typewriter.drain()')
  assert.ok(drainIndex >= 0)
  const settleWindow = source.slice(drainIndex, source.indexOf('let completed', drainIndex))
  assert.doesNotMatch(settleWindow, /await loadSessionMessages\(sessionId, \{ force: true \}\)/)
})

test('assistant text completion drains the typewriter before settling Markdown and history', async () => {
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
  assert.doesNotMatch(textEndHandler, /typewriter\.flush\(\)/)
  assert.match(textEndHandler, /streaming: item\.text !== state\.responseText/)
  assert.match(
    promptCommands,
    /streamState\.responseRenderingStreaming \|\|\s+responseText !== streamState\.responseText/,
  )
  const drainIndex = promptCommands.indexOf('await typewriter.drain()')
  const historyIndex = promptCommands.indexOf('await loadSessionMessages', drainIndex)
  assert.ok(drainIndex >= 0 && historyIndex > drainIndex)
  assert.match(promptCommands, /await typewriter\.drain\(\)\) \|\| !ownsStream\(\)/)
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
  // 空值守卫：没有思考/工具/团队内容时不占位（运行状态由输入框胶囊承载）。
  assert.match(activity, /if \(!thinking && !activities\.length && !team\) return null/)
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
  const queueStart = source.indexOf('const queuePrompt')
  // 从队列处理之后找函数边界，避免命中前面新增的 abortingSessionsRef。
  const queueHandler = source.slice(queueStart, source.indexOf('const abort =', queueStart))
  assert.match(queueHandler, /isEndedSessionQueueError\(error\)/)
  assert.match(queueHandler, /if \(activeStream\) await activeStream\.promise/)
  assert.match(queueHandler, /await loadSessionMessages\(sessionId, \{ force: true \}\)/)
  // 回退必须等待新请求的接收结果；拒绝保留草稿由 replica-chat-behavior 实际调用验证。
  assert.doesNotMatch(queueHandler, /void sendPrompt\(/)
  assert.doesNotMatch(
    queueHandler,
    /notify\(chatErrorMessage\(error\), 'error'\)[\s\S]*isEndedSessionQueueError/,
  )
})

test('shared plan board opens from the composer progress metric', async () => {
  const [dock, session, transcript, controls, board, catalog, styles] = await Promise.all([
    readFile('src/features/chat/ChatDock.tsx', 'utf8'),
    readFile('src/features/chat/FocusSession.tsx', 'utf8'),
    readFile('src/features/chat/FocusTranscript.tsx', 'utf8'),
    readFile('src/features/chat/FocusRuntimeControls.tsx', 'utf8'),
    readFile('src/features/chat/PlanBoard.tsx', 'utf8'),
    readFile('src/features/chat/use-session-catalog.ts', 'utf8'),
    readFile('src/index.css', 'utf8'),
  ])
  assert.match(dock, /resolveSessionPlan\(sessionState, session\)/)
  assert.match(dock, /isPlanActive\(plan, \{ streaming \}\)/)
  assert.match(dock, /plan=\{visiblePlan\}/)
  assert.match(catalog, /plan: planFromPayloadOr\(session, null\)/)
  assert.match(board, /item\.status === 'blocked' \|\| blockedBy\.length > 0/)
  assert.match(board, /view\.status === 'in_progress' && !view\.blocked/)
  assert.match(board, /data-pisper-plan-current/)
  assert.match(board, /list\.scrollTop \+= currentBounds\.bottom - listBounds\.bottom/)
  assert.match(board, /tabIndex=\{views\.length > 4 \? 0 : undefined\}/)
  assert.match(board, /open=\{expanded\}/)
  assert.match(board, /onToggle=\{\(event\) => setExpanded\(event\.currentTarget\.open\)\}/)
  assert.match(board, /data-pisper-plan-current/)
  // 规格 v2 第 2.5 / 4.1 节：运行中的计划在顶栏下方的细进度“计划 2/4”，详情在检查器“计划”标签
  // （InspectorPlan 沿用 PlanBoard）；输入区不再有计划读数，也不直接渲染 PlanBoard。
  const inspectorPlan = await readFile('src/features/chat/InspectorPlan.tsx', 'utf8')
  assert.match(inspectorPlan, /<PlanBoard plan=\{plan\} \/>/)
  assert.match(session, /runtimeFeatureAvailable\(capabilities, 'plans'\)/)
  assert.doesNotMatch(session, /PlanBoard|plan-board-dock|SessionUsageMetrics/)
  assert.doesNotMatch(transcript, /PlanBoard|plan=\{plan\}/)
  assert.doesNotMatch(controls, /PlanBoard|session-plan-progress/)
  assert.doesNotMatch(styles, /\.plan-board-dock/)
  assert.match(board, /views\.length > 4 \? ' is-scrollable[^']*overflow-y-auto/)
  assert.doesNotMatch(board, /unblocks:/)
})

test('bash tool output stays multiline in a bounded theme-aware result block', async () => {
  const [runtime, dispatcher, activity, terminalOutput, packageJson] = await Promise.all([
    readFile('runtime/runtime/agent-runtime.mjs', 'utf8'),
    readFile('src/features/chat/stream-event-dispatch.ts', 'utf8'),
    readFile('src/features/chat/AgentRunActivity.tsx', 'utf8'),
    readFile('src/lib/terminal-output.ts', 'utf8'),
    readFile('package.json', 'utf8'),
  ])
  assert.match(
    runtime,
    /const rawOutput = liveThinkingTail\(textFromContent\(event\.partialResult\?\.content\)\)/,
  )
  assert.match(
    runtime,
    /const outputPatch = event\.toolName === 'bash' \? \{ output: rawOutput \} : \{\}/,
  )
  assert.match(runtime, /const resultOutput =\s*event\.toolName === 'bash'/)
  assert.match(dispatcher, /data\.output !== undefined \? \{ output: data\.output \} : \{\}/)
  assert.doesNotMatch(activity, /components\/ai-elements\/terminal/)
  assert.match(activity, /activity\.name === 'bash'/)
  assert.match(activity, /terminalDisplayOutput\(output\)/)
  assert.match(activity, /className="agent-run-command-output[^"\n]*"/)
  assert.match(terminalOutput, /MAX_TERMINAL_DISPLAY_CHARS = 4_000/)
  assert.match(activity, /agent-run-command-output[^"\n]*\[&_>_pre\]:max-h-\[112px\]/)
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
  // 隐藏面板不渲染 SessionPanel，其加载消息/思考状态的 effect 随卸载跳过。
  assert.match(dock, /if \(sessionId\) void loadMessages\?\.\(sessionId/)
  assert.match(dispatcher, /tools: pushCurrentActivity\(current\.tools, activity\)/)
  assert.match(ascii, /new ResizeObserver\(draw\)/)
  assert.doesNotMatch(ascii, /requestAnimationFrame|setInterval/)
})
