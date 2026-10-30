// Agent 运行活动面板：思考过程是一枚可展开的“思考”芯片；工具活动是一张工具组卡片——
// 组标题“3 个操作 · 2 完成 · 1 运行中”，每行 36px：状态字形 + 等宽动词 + 路径/命令 + 耗时 + 展开箭头，
// 实时跟随 SSE 流更新；bash 行可展开为凹陷的终端输出井，正在运行的那条默认展开并用运行青浅底标出。
import {
  lazy,
  memo,
  Suspense,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { Activity, ChevronRight, Lightbulb } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { Plan } from '@/components/ai-elements/plan'
import { Task } from '@/components/ai-elements/task'
import MarkdownMessage from '@/components/MarkdownMessage'
import { planFromActivity } from '@/lib/plan-protocol'
import { terminalDisplayOutput } from '@/lib/terminal-output'
import type { EntityRecord, TeamCommunication, TeamState, TeamTask } from '@/types/chat'
import { ComputerUseLiveMirror } from './ComputerUseLiveMirror'
import { ChatRequestNotice } from './ChatRequestNotice'
import { latestComputerUseTarget } from './computer-use-live-support.ts'
import { useSessionInspectorStore } from './session-inspector-store'
import { SessionInspectorContext } from './session-workspace-context'
import {
  agentActivityTitle,
  commandExitCode,
  compactionText,
  formatStepDuration,
  planChangeText,
  planProgress,
  stepSpanMs,
  toolActivityLabel,
  toolCompletedLabel,
  type Translate,
} from './run-activity-presentation'
import {
  activityRenderKey,
  activityScrollVersion,
  agentActivityState,
  primaryRunActivity,
  runDurationMs,
} from './run-activity'

const EMPTY_LIST: EntityRecord[] = []
// 日志行几何（规格 4.3）：桌面 36px、触屏与窄屏 44px；所有行共用，保证状态字形与动词列对齐。
const LOG_ROW_CLASS =
  'flex min-h-[36px] min-w-0 items-center gap-[10px] px-[12px] py-[5px] text-[13px] max-[650px]:min-h-11 pointer-coarse:min-h-11 [.agent-run-activity.compact_&]:min-h-[30px]'
// 可展开行（details > summary）：悬停/展开都只染一级次表面，焦点用 2px 内描边避免被卡片裁切。
const SUMMARY_BASE_CLASS =
  'cursor-pointer list-none transition-colors duration-(--d-fast) ease-precise focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus [&::-webkit-details-marker]:hidden'
const LOG_SUMMARY_CLASS = `${SUMMARY_BASE_CLASS} hover:bg-surface-subtle [details[open]>&]:bg-surface-subtle`
// 正在运行的工具行用运行青浅底（--st-run-soft），展开后保持同色，不被次表面覆盖。
const RUNNING_SUMMARY_CLASS = `${SUMMARY_BASE_CLASS} bg-st-run-soft`
const DISCLOSURE_CLASS =
  'agent-run-disclosure size-[14px] flex-none text-content-muted transition-transform duration-(--d-fast) ease-precise [details[open]>summary_&]:rotate-90'
const AnimatedList = lazy(() =>
  import('@/components/react-bits/AnimatedList').then((module) => ({
    default: module.AnimatedList,
  })),
)

function teamTaskStatusLabel(status: unknown, t: Translate) {
  if (status === 'queued') return t('chat:agentRunActivity.teamTaskQueued')
  if (status === 'starting') return t('chat:agentRunActivity.teamTaskStarting')
  if (status === 'running') return t('chat:agentRunActivity.teamTaskRunning')
  if (status === 'completed') return t('chat:agentRunActivity.teamTaskCompleted')
  if (status === 'failed') return t('chat:agentRunActivity.teamTaskFailed')
  if (status === 'interrupted') return t('chat:agentRunActivity.teamTaskInterrupted')
  if (status === 'blocked') return t('chat:agentRunActivity.teamTaskBlocked')
  return t('chat:agentRunActivity.teamTaskStatusUpdated')
}

export type AgentRunActivityProps = {
  streaming?: boolean
  text?: string
  thinkingText?: string
  currentActivity?: EntityRecord | null
  team?: TeamState | null
  activityFeed?: EntityRecord[]
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  startedAt?: string | null
  lastActivityAt?: string | null
  finishedAt?: string | null
  compact?: boolean
  tools?: EntityRecord[]
}

type ActivityPresentationContext = {
  t: Translate
  streaming?: boolean
  text?: string
  thinkingText?: string
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  lastActivityAt?: string | null
  now: number
}

type ActivityPresentation = {
  tone: string
  title: string
  detail: string
  output: string
  command: boolean
  startedAt: unknown
  changes: EntityRecord[]
}

function useRunActivityClock(streaming?: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    setNow(Date.now())
    if (!streaming) return undefined
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [streaming])
  return now
}

function RunDurationLabel({
  startedAt,
  finishedAt,
  streaming,
}: {
  startedAt?: string | null
  finishedAt?: string | null
  streaming?: boolean
}) {
  // 每秒变化只留在这一小块文本中，避免整棵 SSE 活动树随计时器重复渲染。
  const now = useRunActivityClock(streaming)
  const elapsed = runDurationMs(startedAt, finishedAt, now)
  // 缺少开始时间的历史记录没有可信时长，不以 0 冒充。
  if (!elapsed && !streaming) return null
  return (
    <span className="agent-run-duration flex-none text-[12px] whitespace-nowrap text-content-muted tabular-nums">
      {formatStepDuration(elapsed, Boolean(streaming))}
    </span>
  )
}

function cleanInline(value: unknown) {
  return String(value || '')
    .replace(/\s+/g, ' ')
    .trim()
}

function toolDetail(tool?: EntityRecord | null) {
  const args = tool?.args || {}
  if (tool?.name === 'bash') return { text: String(args.command || '').trim(), command: true }
  if (['read', 'edit', 'write', 'ls'].includes(tool?.name)) return { text: cleanInline(args.path) }
  if (tool?.name === 'grep')
    return {
      text: [args.pattern ? `“${cleanInline(args.pattern)}”` : '', cleanInline(args.path)]
        .filter(Boolean)
        .join(' · '),
    }
  if (tool?.name === 'find')
    return { text: [cleanInline(args.pattern), cleanInline(args.path)].filter(Boolean).join(' · ') }
  if (tool?.name === 'browser_automation')
    return {
      text: [cleanInline(args.action), cleanInline(args.url || args.selector)]
        .filter(Boolean)
        .join(' · '),
    }
  // computer use 工具：显示观察模式或动作摘要，帮助用户辨认当前在哪个窗口干什么。
  if (tool?.name === 'observe_ui') return { text: cleanInline(args.root || args.mode) }
  if (tool?.name === 'act_ui')
    return {
      text: Array.isArray(args.actions)
        ? args.actions
            .slice(0, 3)
            .map((action: EntityRecord) => cleanInline(action?.action))
            .filter(Boolean)
            .join(' · ')
        : '',
    }
  if (tool?.name === 'search_ui') return { text: cleanInline(args.text || args.role) }
  if (tool?.name === 'find_roots') return { text: cleanInline(args.app || args.text) }
  if (tool?.name === 'ocr_ui') return { text: cleanInline(args.language) }
  if (tool?.name === 'spawn_agent') return { text: cleanInline(args.taskName) }
  if (['send_message', 'followup_task', 'wait_agent', 'interrupt_agent'].includes(tool?.name))
    return { text: cleanInline(args.target) }
  if (tool?.name === 'generate_visual') return { text: cleanInline(args.outputName || args.kind) }
  if (tool?.name === 'discover_tools') return { text: cleanInline(args.query) }
  return { text: '' }
}

function activityPresentation(
  activity: EntityRecord,
  {
    t,
    streaming,
    text,
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now,
  }: ActivityPresentationContext,
): ActivityPresentation {
  let tone = 'running'
  let title = t('chat:agentRunActivity.understandingTheTask')
  let detail = notice || ''
  let output = ''
  let command = false
  let startedAt = activity.startedAt
  let changes = EMPTY_LIST

  if (activity.type === 'tool') {
    const failed = activity.status === 'error'
    const completed = activity.status === 'done'
    tone = failed ? 'failed' : completed ? 'completed' : 'running'
    title = failed
      ? t('chat:agentRunActivity.toolFailed', {
          tool: toolActivityLabel(activity.name, t),
        })
      : completed
        ? toolCompletedLabel(activity.name, t)
        : toolActivityLabel(activity.name, t)
    const toolInfo = toolDetail(activity)
    detail = toolInfo.text
    command = Boolean(toolInfo.command)
    output = cleanInline(activity.message)
  } else if (activity.type === 'plan') {
    tone = 'plan'
    const activityPlan = planFromActivity(activity)
    title = activityPlan?.items?.length
      ? t('chat:agentRunActivity.planUpdated')
      : t('chat:agentRunActivity.planCleared')
    changes = activity.changes || EMPTY_LIST
    detail = changes.length
      ? t('chat:agentRunActivity.countPlanChanges', { count: changes.length })
      : planProgress(activityPlan, t)
  } else if (activity.type === 'agent') {
    const agent = activity.agent || {}
    const name = cleanInline(agent.taskName) || t('chat:agentRunActivity.subagent')
    const state = agentActivityState(agent.status)
    title = agentActivityTitle(agent.status, name, t)
    tone = state.tone
    const nestedTool =
      agent.currentActivity?.type === 'tool'
        ? toolDetail(agent.currentActivity).text || agent.currentActivity.name
        : ''
    detail = nestedTool || cleanInline(agent.error || agent.message)
    output = agent.status === 'running' ? cleanInline(agent.output) : ''
    startedAt = agent.startedAt || startedAt
  } else if (activity.type === 'communication') {
    const communication = activity.communication || {}
    tone = 'running'
    title = t('chat:agentRunActivity.teamMessageActivity')
    detail = t('chat:agentRunActivity.teamMessageDetail', {
      from: communication.fromTaskName || communication.fromAgentId || 'lead',
      to: communication.toTaskName || communication.toAgentId || '',
    })
    output = cleanInline(communication.message)
  } else if (activity.type === 'compaction') {
    tone = 'compacting'
    title = t('chat:agentRunActivity.compactingContext')
    detail = compactionText(activity.compaction || compaction, t)
  } else if (activity.type === 'retry') {
    tone = 'waiting'
    title = t('chat:agentRunActivity.retryingRequest')
    detail = activity.summary || notice || ''
  } else {
    const inactiveMs = Math.max(0, now - new Date(lastActivityAt || now).getTime())
    if (error) {
      tone = 'failed'
      title = t('chat:agentRunActivity.thisRunFailed')
      detail = error
    } else if (stopped) {
      tone = 'stopped'
      title = t('chat:agentRunActivity.runStopped')
    } else if (!streaming) {
      tone = 'completed'
      title = t('chat:agentRunActivity.reasoningCompleted')
      detail = cleanInline(thinkingText)
    } else if (inactiveMs >= 10_000) {
      tone = 'waiting'
      title = t('chat:agentRunActivity.waitingForTheModel')
      detail = t('chat:agentRunActivity.noNewProgressForCountS', {
        count: Math.floor(inactiveMs / 1000),
      })
    } else if (activity.stage === 'starting') title = t('chat:agentRunActivity.startingTheModel')
    else if (activity.stage === 'responding')
      title = t('chat:agentRunActivity.preparingTheResponse')
    else if (activity.stage === 'processing_result')
      title = t('chat:agentRunActivity.processingToolResult')
    else if (activity.stage === 'waiting_retry') title = t('chat:agentRunActivity.waitingToRetry')
    else if (activity.stage === 'finalizing') title = t('chat:agentRunActivity.finalizingTheRun')
    else if (activity.stage === 'working') title = t('chat:agentRunActivity.advancingTheTask')
    else
      title = String(thinkingText || '').trim()
        ? t('chat:agentRunActivity.reasoningAboutTheNextStep')
        : String(text || '').trim()
          ? t('chat:agentRunActivity.preparingTheResponse')
          : t('chat:agentRunActivity.understandingTheTask')
    if (String(thinkingText || '').trim()) detail = cleanInline(thinkingText)
  }

  return { tone, title, detail, output, command, startedAt, changes }
}

// 输入框状态胶囊与转录区活动行共用同一份状态推导，避免两处文案漂移。
// oxlint-disable-next-line react/only-export-components
export function runActivityStatusLabel(
  {
    streaming,
    text,
    currentActivity,
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
  }: Omit<ActivityPresentationContext, 't' | 'now'> & {
    currentActivity?: EntityRecord | null
  },
  t: Translate,
): string {
  if (!streaming) return ''
  const activity = primaryRunActivity({
    currentActivity,
    compaction,
    text,
    thinkingText,
    lastActivityAt,
  })
  return activityPresentation(activity, {
    t,
    streaming,
    text,
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now: Date.now(),
  }).title
}

// 状态字形（规格 6.7 的共用 StatusGlyph）：形状 + 状态色同时表达结果，不只依赖色相区分成功与失败；
// 运行中的弧形字形旋转（减少动效时停转），颜色只取 --st-* 状态族，不随强调色变化。
const TONE_GLYPHS: Record<string, { kind: StatusGlyphKind; color: string }> = {
  running: { kind: 'run', color: 'text-st-run' },
  compacting: { kind: 'run', color: 'text-st-run' },
  completed: { kind: 'done', color: 'text-st-done' },
  plan: { kind: 'done', color: 'text-st-done' },
  failed: { kind: 'failed', color: 'text-st-fail' },
  stopped: { kind: 'stopped', color: 'text-st-queue' },
  waiting: { kind: 'queued', color: 'text-st-queue' },
}

function ToneGlyph({ tone, spin = true }: { tone: string; spin?: boolean }) {
  const glyph = TONE_GLYPHS[tone] ?? { kind: 'queued' as const, color: 'text-st-queue' }
  return (
    <span className="agent-run-status-icon grid size-[18px] flex-none place-items-center">
      <StatusGlyph
        status={glyph.kind}
        spin={spin && glyph.kind === 'run'}
        className={`size-[15px] ${glyph.color}`}
      />
    </span>
  )
}

// 工具组标题的计数：只按活动自身的状态归类（完成 / 运行中 / 失败），不另外推导；
// 运行结束后残留的 running 记录按完成计，避免历史回复里出现“运行中”。
function activityOutcome(activity: EntityRecord, streaming?: boolean) {
  const status = activity.type === 'agent' ? activity.agent?.status : activity.status
  if (status === 'error' || status === 'failed') return 'failed'
  if (streaming && ['running', 'starting', 'queued'].includes(String(status))) return 'running'
  if (streaming && activity.type === 'compaction') return 'running'
  return 'done'
}

function activityGroupCounts(activities: EntityRecord[], streaming?: boolean) {
  const counts = { total: activities.length, done: 0, running: 0, failed: 0 }
  for (const activity of activities) counts[activityOutcome(activity, streaming)] += 1
  return counts
}

function ActivityElement({
  activity,
  className,
  children,
}: {
  activity: EntityRecord
  className: string
  children: ReactNode
}) {
  if (activity.type === 'tool') {
    return (
      <div className={className} data-pisper-activity-type="tool">
        {children}
      </div>
    )
  }
  if (activity.type === 'plan') {
    return (
      <Plan
        className={`${className} !gap-0 !rounded-none !border-0 !bg-transparent !p-0 !shadow-none`}
        data-pisper-activity-type="plan"
      >
        {children}
      </Plan>
    )
  }
  if (activity.type === 'agent') {
    return (
      <Task className={className} data-pisper-activity-type="agent">
        {children}
      </Task>
    )
  }
  return (
    <div className={className} data-pisper-activity-type={activity.type}>
      {children}
    </div>
  )
}

// 终端行着色只识别输出里真实存在的标记：行首 PASS/FAIL 变为浅色标签；失败要点（● ✗ ×）、
// npm 的 ERR!、Error: 前缀与运行时追加的退出码行标红，行首 ✓ / ✔ 标绿，其余原样显示。
// 颜色只取 --st-* 状态族：红只表示失败，绿只表示通过。
const FAILURE_LINE =
  /^\s*(?:●|✗|✕|×|npm ERR!|ERR!|(?:Error|TypeError|SyntaxError|ReferenceError)\b:?)|^Command exited with code \d+/
const SUCCESS_LINE = /^\s*[✓✔]/

function TerminalLine({ line }: { line: string }) {
  const tag = /^(PASS|FAIL)(?=\s|$)/.exec(line)?.[1]
  if (tag)
    return (
      <span>
        <b
          className={`mr-[6px] rounded-[3px] px-[5px] text-[11.5px] font-semibold tracking-[.02em] ${tag === 'PASS' ? 'bg-st-done-soft text-st-done' : 'bg-st-fail-soft text-st-fail'}`}
        >
          {tag}
        </b>
        {line.slice(tag.length).trimStart()}
      </span>
    )
  const tone = FAILURE_LINE.test(line)
    ? 'text-st-fail'
    : SUCCESS_LINE.test(line)
      ? 'text-st-done'
      : undefined
  return <span className={tone}>{line}</span>
}

function CommandOutput({
  output,
  command,
  streaming,
  t,
  children,
}: {
  output: unknown
  command: string
  streaming: boolean
  t: Translate
  children: ReactNode
}) {
  const display = terminalDisplayOutput(output)
  const outputRef = useRef<HTMLPreElement>(null)

  useEffect(() => {
    if (!streaming) return undefined
    const frame = window.requestAnimationFrame(() => {
      const node = outputRef.current
      if (node) node.scrollTop = node.scrollHeight
    })
    return () => window.cancelAnimationFrame(frame)
  }, [display.text, streaming])

  // 运行中的命令即使还没有输出也展开，先露出完整命令；结束后没有输出的行不再可展开。
  if (!display.text.trim() && !(streaming && command))
    return <div className={LOG_ROW_CLASS}>{children}</div>

  // 行本身就是展开开关；展开后在凹陷的输出井（--well-*）中显示等宽输出，左侧细引导线与状态字形对齐。
  return (
    <details
      className="agent-run-command-output [.agent-run-feed:has(&[open])]:max-h-[320px] [&_>_pre]:max-h-[112px] [&_>_pre]:overflow-auto [&_>_pre]:m-0 [&_>_pre]:relative [&_>_pre]:border-t [&_>_pre]:border-well-stroke [&_>_pre]:bg-well [&_>_pre]:py-[8px] [&_>_pre]:pr-[14px] [&_>_pre]:pl-[40px] [&_>_pre]:font-mono [&_>_pre]:text-[12px] [&_>_pre]:leading-[1.7] [&_>_pre]:text-well-text [&_>_pre]:whitespace-pre-wrap [&_>_pre]:[overflow-wrap:anywhere] [&_>_pre>span]:block [&_>_pre>span]:min-h-[1.7em] [&_>_pre]:before:absolute [&_>_pre]:before:top-0 [&_>_pre]:before:bottom-[12px] [&_>_pre]:before:left-[20px] [&_>_pre]:before:w-px [&_>_pre]:before:bg-well-stroke max-[650px]:[&_>_pre]:pl-[12px] max-[650px]:[&_>_pre]:before:hidden min-w-0"
      data-truncated={display.truncated || undefined}
      data-running={streaming || undefined}
      open={streaming || undefined}
    >
      <summary
        className={`${LOG_ROW_CLASS} ${streaming ? RUNNING_SUMMARY_CLASS : LOG_SUMMARY_CLASS}`}
      >
        {children}
        <span className="sr-only">
          {streaming
            ? t('chat:agentRunActivity.liveCommandOutput')
            : t('chat:agentRunActivity.commandOutput')}
        </span>
        <ChevronRight className={DISCLOSURE_CLASS} aria-hidden="true" />
      </summary>
      <pre ref={outputRef}>
        {command ? (
          <span className="text-well-text">
            <span className="text-well-muted select-none">$ </span>
            {command}
          </span>
        ) : null}
        {display.text
          ? display.text.split('\n').map((line, index) => <TerminalLine key={index} line={line} />)
          : null}
      </pre>
    </details>
  )
}

// 工具活动预览图：computer use / 浏览器截图等工具的“实时窗口”画面。
// 每次观察/操作后由运行时补发 previewImage，随活动卡片就地更新；
// 点击可在新标签页查看原图（内联下载响应本身就是图像）。
function PreviewImage({ image, t }: { image: EntityRecord; t: Translate }) {
  const url = String(image.url || '')
  if (!url) return null
  return (
    <a
      className="agent-run-tool-preview block w-full overflow-hidden rounded-sm border border-stroke-soft bg-surface-subtle"
      href={url}
      target="_blank"
      rel="noreferrer"
      title={String(image.name || '')}
    >
      <img
        alt={String(image.name || t('chat:agentRunActivity.toolPreviewAlt'))}
        className="block max-h-[168px] w-full object-contain object-top"
        loading="lazy"
        src={url}
      />
    </a>
  )
}

type ActivityCardProps = {
  activity: EntityRecord
  latest: boolean
  t: Translate
  streaming?: boolean
  text?: string
  thinkingText?: string
  compaction?: EntityRecord | null
  error?: string
  stopped?: boolean
  notice?: string
  lastActivityAt?: string | null
}

function activityCardPropsEqual(prev: ActivityCardProps, next: ActivityCardProps) {
  if (
    prev.activity !== next.activity ||
    prev.latest !== next.latest ||
    prev.t !== next.t ||
    prev.streaming !== next.streaming
  )
    return false
  if (['tool', 'plan', 'agent'].includes(next.activity.type)) return true
  return (
    prev.text === next.text &&
    prev.thinkingText === next.thinkingText &&
    prev.compaction === next.compaction &&
    prev.error === next.error &&
    prev.stopped === next.stopped &&
    prev.notice === next.notice &&
    prev.lastActivityAt === next.lastActivityAt
  )
}

const ActivityCard = memo(function ActivityCard({
  activity,
  latest,
  t,
  streaming,
  text,
  thinkingText,
  compaction,
  error,
  stopped,
  notice,
  lastActivityAt,
}: ActivityCardProps) {
  const presentation = activityPresentation(activity, {
    t,
    streaming,
    text,
    thinkingText,
    compaction,
    error,
    stopped,
    notice,
    lastActivityAt,
    now: Date.now(),
  })
  // 每条带输出的 bash 行都可展开查看终端输出；运行中的那条默认展开并跟随滚动（还没有输出时先露出完整命令）。
  const runningTool = Boolean(
    streaming && activity.type === 'tool' && activity.status === 'running',
  )
  const showCommandOutput = activity.name === 'bash' && Boolean(activity.output || runningTool)
  if (activity.type === 'retry') {
    return (
      <ChatRequestNotice
        title={activity.summary || t('chat:agentRunActivity.retryingRequest')}
        error={String(activity.message || '')}
        pending={Boolean(streaming && latest)}
        className="rounded-none border-0 bg-transparent"
      />
    )
  }
  const verb = activity.type === 'tool' ? String(activity.name || '') : ''
  const exitCode = commandExitCode(activity)
  const span = stepSpanMs(activity)
  const failed = presentation.tone === 'failed'
  const outputLine =
    !showCommandOutput && presentation.output && presentation.output !== presentation.detail
      ? presentation.output
      : ''
  // 工具行：等宽动词 + 路径/命令；非工具活动（计划、子 Agent、团队消息）用标题 + 说明。
  const row = (
    <>
      <ToneGlyph tone={presentation.tone} spin={Boolean(streaming)} />
      {verb ? (
        <>
          <span
            className={`agent-run-verb max-w-[40%] min-w-[34px] flex-none truncate font-mono text-[12px] font-semibold ${runningTool ? 'text-st-run' : 'text-content-secondary'}`}
          >
            {verb}
          </span>
          <span
            className={`agent-run-arg min-w-0 flex-1 truncate font-mono text-[12.5px] ${presentation.detail ? 'text-foreground' : 'text-content-muted'}`}
            title={presentation.detail || presentation.title}
          >
            {presentation.detail || presentation.title}
          </span>
          {presentation.detail ? <span className="sr-only">{presentation.title}</span> : null}
        </>
      ) : (
        <span
          className="agent-run-arg min-w-0 flex-1 truncate"
          title={[presentation.title, presentation.detail].filter(Boolean).join(' · ')}
        >
          <span className="text-foreground">{presentation.title}</span>
          {presentation.detail ? (
            <span className="text-content-muted"> · {presentation.detail}</span>
          ) : null}
        </span>
      )}
      {exitCode !== null && (
        <span className="flex-none font-mono text-[12px] font-medium text-st-fail">
          {t('chat:agentRunActivity.exitCode', { code: exitCode })}
        </span>
      )}
      {span !== null ? (
        <span className="agent-run-elapsed min-w-[44px] flex-none text-right font-mono text-[12px] text-content-muted tabular-nums">
          {formatStepDuration(span)}
        </span>
      ) : runningTool && activity.startedAt ? (
        // 运行中的工具行显示已运行多久：每秒刷新只留在这一小段计时文字里。
        <RunDurationLabel startedAt={String(activity.startedAt)} streaming />
      ) : null}
    </>
  )
  return (
    <ActivityElement
      activity={activity}
      className={`agent-run-summary flex w-full min-w-0 flex-none flex-col [transition:background_var(--d-fast)_var(--ease)] ${presentation.tone} ${latest ? 'current [.agent-run-feed.live_&]:bg-surface-subtle' : ''}`}
    >
      {showCommandOutput ? (
        <CommandOutput
          output={activity.output}
          command={presentation.detail}
          streaming={runningTool}
          t={t}
        >
          {row}
        </CommandOutput>
      ) : (
        <div className={`${LOG_ROW_CLASS} ${runningTool ? 'bg-st-run-soft' : ''}`}>
          {row}
          {/* 不可展开的行保留箭头列宽，让耗时列与可展开行对齐。 */}
          <span className="size-[14px] flex-none" aria-hidden="true" />
        </div>
      )}
      {(presentation.changes.length > 0 || outputLine || activity.previewImage?.url) && (
        <div className="flex min-w-0 flex-col gap-[4px] pr-[12px] pb-[8px] pl-[40px] text-[12px] leading-[1.45]">
          {presentation.changes.length > 0 && (
            <span className="agent-run-plan-changes flex min-w-0 flex-col gap-[1px] border-l border-stroke pl-[8px] text-content-secondary">
              {presentation.changes.slice(0, 4).map((change) => (
                <small
                  className="truncate text-[12px]"
                  key={`${change.id}-${change.kind}-${change.status}`}
                >
                  {planChangeText(change, t)}
                </small>
              ))}
              {presentation.changes.length > 4 && (
                <small className="text-[12px] text-content-muted">
                  {t('chat:agentRunActivity.countMoreChanges', {
                    count: presentation.changes.length - 4,
                  })}
                </small>
              )}
            </span>
          )}
          {outputLine ? (
            <small
              className={`truncate text-[12px] ${failed ? 'text-st-fail' : 'text-content-muted'}`}
              title={outputLine}
            >
              {outputLine}
            </small>
          ) : null}
          {activity.previewImage && activity.previewImage.url ? (
            <PreviewImage image={activity.previewImage} t={t} />
          ) : null}
        </div>
      )}
    </ActivityElement>
  )
}, activityCardPropsEqual)

function AgentRunActivity({
  streaming,
  text,
  thinkingText,
  team,
  activityFeed = EMPTY_LIST,
  tools = EMPTY_LIST,
  compaction,
  error,
  stopped,
  notice,
  startedAt,
  lastActivityAt,
  finishedAt,
  compact = false,
}: AgentRunActivityProps) {
  const { t } = useI18n()
  const thinking = String(thinkingText || '').trim()
  const thinkingScrollRef = useRef<HTMLDivElement>(null)
  const liveFeedRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!thinking) return undefined
    const frame = window.requestAnimationFrame(() => {
      const node = thinkingScrollRef.current
      if (node) node.scrollTop = node.scrollHeight
    })
    return () => window.cancelAnimationFrame(frame)
  }, [thinking])

  const activities = activityFeed.length ? activityFeed : tools
  // 会话页检查器的“预览”标签可见时，实时窗口镜像由那里承担：同一个窗口不同时开两条帧流。
  const inspector = useContext(SessionInspectorContext)
  const inspectorTab = useSessionInspectorStore((state) => state.tab)
  const mirrorInInspector = Boolean(inspector?.open && inspectorTab === 'preview')
  // computer use 实时镜像跟随 agent 当前操作的目标窗口；仅流式期间激活，
  // 且镜像回退到同一活动的静态预览图。
  const computerUseTarget =
    streaming && !compact && !mirrorInInspector ? latestComputerUseTarget(activities) : null
  const activityVersion = activityScrollVersion(activities)
  const teamTasks: TeamTask[] = Array.isArray(team?.tasks) ? team.tasks : []
  const teamCompleted = Number(team?.completedTaskCount ?? 0)
  const teamTotal = Number(team?.taskCount ?? teamTasks.length)
  const teamUsed = Number(team?.tokenUsed ?? team?.tokensUsed ?? 0)
  const teamBudget = Number(team?.tokenBudget ?? team?.teamTokenBudget ?? 0)
  const teamCommunications: TeamCommunication[] = Array.isArray(team?.communications)
    ? team.communications
    : []
  const teamProgress =
    teamTotal > 0 ? Math.min(100, Math.round((teamCompleted / teamTotal) * 100)) : 0

  useEffect(() => {
    if (!streaming || !activities.length) return undefined
    const frame = window.requestAnimationFrame(() => {
      const node = liveFeedRef.current
      if (node) node.scrollTop = node.scrollHeight
    })
    return () => window.cancelAnimationFrame(frame)
  }, [activityVersion, activities.length, streaming])
  // 没有任何可见内容时不占位：运行状态由输入框的呼吸灯胶囊承载。
  if (!thinking && !activities.length && !team) return null

  const counts = activityGroupCounts(activities, streaming)
  const renderActivityCards = (items: EntityRecord[]) =>
    items.map((activity, index) => (
      <ActivityCard
        activity={activity}
        compaction={compaction}
        error={error}
        key={activityRenderKey(activity, index)}
        lastActivityAt={lastActivityAt}
        latest={index === items.length - 1}
        notice={notice}
        stopped={stopped}
        streaming={streaming}
        t={t}
        text={text}
        thinkingText={thinking ? '' : thinkingText}
      />
    ))
  const activityCards = renderActivityCards(activities)
  const showTeamPanel = Boolean(
    team &&
    (teamTasks.length > 0 ||
      teamCommunications.length > 0 ||
      (team.blockers?.length ?? 0) > 0 ||
      (team.conflicts?.length ?? 0) > 0 ||
      Boolean(team.scriptPath) ||
      Boolean(team.summary?.text)),
  )
  const showLog = Boolean(showTeamPanel || activities.length > 0)
  // 会话路由随页加载看板命名空间，工具组标题的计数文案在那里（不占入口常驻的 chat 命名空间）。
  const groupParts = [
    t('board:transcript.operations', { count: counts.total }),
    counts.done ? t('board:transcript.done', { count: counts.done }) : '',
    counts.running ? t('board:transcript.running', { count: counts.running }) : '',
    counts.failed ? t('board:transcript.failed', { count: counts.failed }) : '',
  ].filter(Boolean)

  return (
    <section
      className={`agent-run-activity flex w-full flex-col items-start gap-[10px] [margin:2px_0_14px] text-foreground [&.compact]:m-[4px_0_0_40px] ${compact ? 'compact' : ''}`}
      aria-live="polite"
    >
      {thinking && (
        // 思考过程收成一枚芯片（规格 4.3 的“思考”芯片）：默认折叠，展开后在芯片下方显示推理文本。
        <details
          className={`agent-thinking-window group/thinking w-full min-w-0 ${streaming ? 'running' : 'completed'}`}
          data-pisper-activity-type="reasoning"
        >
          <summary
            className={`agent-thinking-head inline-flex h-7 max-w-full items-center gap-[6px] rounded-sm pr-[8px] pl-[6px] text-[12px] text-content-secondary ring-1 ring-stroke ring-inset ${SUMMARY_BASE_CLASS} hover:bg-surface-hover max-[650px]:h-11 pointer-coarse:h-11`}
            aria-label={t('chat:agentRunActivity.toggleReasoning')}
          >
            {streaming ? (
              <StatusGlyph status="run" className="size-[14px] text-st-run" />
            ) : (
              <Lightbulb className="size-[14px] flex-none text-content-muted" strokeWidth={1.75} />
            )}
            <strong className="flex min-w-0 items-center truncate font-medium">
              {streaming
                ? t('chat:agentRunActivity.reasoningInProgress')
                : t('chat:agentRunActivity.reasoningCompleted')}
              {streaming ? (
                <span
                  className="agent-thinking-dots [&_i]:w-[3px] [&_i]:h-[3px] [&_i]:rounded-[50%] [&_i]:bg-[currentColor] [&_i]:[animation:agent-thinking-dot_1.15s_ease-in-out_infinite] [&_i:nth-child(2)]:[animation-delay:.14s] [&_i:nth-child(3)]:[animation-delay:.28s] inline-flex items-end gap-[2px] [margin-left:5px]"
                  aria-hidden="true"
                >
                  <i />
                  <i />
                  <i />
                </span>
              ) : null}
            </strong>
            {/* 没有工具组时，这一轮的总时长挂在思考芯片上；有工具组时放在组标题里，不重复。 */}
            {!activities.length && (
              <RunDurationLabel
                startedAt={startedAt}
                finishedAt={finishedAt}
                streaming={streaming}
              />
            )}
            <ChevronRight className={DISCLOSURE_CLASS} aria-hidden="true" />
          </summary>
          <div
            ref={thinkingScrollRef}
            className="agent-thinking-scroll mt-[8px] max-h-[190px] overflow-auto rounded-md border border-stroke-soft bg-surface-subtle px-[14px] py-[9px] text-content-secondary [scroll-behavior:smooth]"
          >
            <MarkdownMessage streaming={streaming}>{thinking}</MarkdownMessage>
          </div>
        </details>
      )}
      {showLog && (
        // 工具组卡片：组标题 + 细线分隔的 36px 工具行；团队面板与工具行共享同一张 1px 描边卡片。
        <div className="agent-run-log w-full overflow-hidden rounded-md border border-stroke bg-solid [&>*+*]:border-t [&>*+*]:border-stroke-soft">
          {team && showTeamPanel && (
            <details
              className="agent-team-panel min-w-0"
              key={String(team.id || team.scriptPath || 'team')}
            >
              <summary className={`${LOG_ROW_CLASS} ${LOG_SUMMARY_CLASS} justify-between`}>
                <span className="flex min-w-0 items-center gap-[10px]">
                  <span
                    className="grid size-[18px] flex-none place-items-center"
                    aria-hidden="true"
                  >
                    <span
                      className={`team-status-dot h-[7px] w-[7px] shrink-0 rounded-full ${
                        team.status === 'complete'
                          ? 'bg-success'
                          : team.status === 'stalled' || team.status === 'budget_limited'
                            ? 'bg-warning-strong'
                            : team.status === 'paused'
                              ? 'bg-content-muted'
                              : 'bg-star [animation:agent-status-pulse_1.6s_ease-in-out_infinite]'
                      }`}
                    />
                  </span>
                  <span className="min-w-0 truncate font-medium text-content-secondary">
                    {team.status === 'complete'
                      ? t('chat:agentRunActivity.teamComplete')
                      : team.status === 'budget_limited'
                        ? t('chat:agentRunActivity.teamBudgetLimited')
                        : team.status === 'paused'
                          ? t('chat:agentRunActivity.teamPaused')
                          : team.status === 'stalled'
                            ? t('chat:agentRunActivity.teamStalled')
                            : t('chat:agentRunActivity.teamActive')}
                  </span>
                  {teamTotal > 0 && (
                    <small className="shrink-0 text-[12px] text-content-muted tabular-nums">
                      {teamCompleted}/{teamTotal} · {teamProgress}%
                    </small>
                  )}
                </span>
                <span className="flex shrink-0 items-center gap-[8px]">
                  {teamBudget > 0 && (
                    <small className="text-[12px] whitespace-nowrap text-content-muted tabular-nums">
                      {t('chat:agentRunActivity.teamBudget', {
                        used: teamUsed,
                        budget: teamBudget,
                      })}
                    </small>
                  )}
                  <ChevronRight className={DISCLOSURE_CLASS} aria-hidden="true" />
                </span>
              </summary>
              {teamTotal > 0 && (
                <div
                  aria-hidden="true"
                  className="team-progress-track mx-[12px] mb-[6px] h-[4px] overflow-hidden rounded-full bg-surface-active"
                >
                  <div
                    className="team-progress-fill h-full rounded-full bg-content-secondary [transition:width_var(--d-base)_var(--ease)]"
                    style={{
                      width: `${teamProgress}%`,
                    }}
                  />
                </div>
              )}
              <div className="grid max-h-[min(42vh,360px)] grid-cols-[repeat(auto-fit,minmax(190px,1fr))] gap-[6px] overflow-y-auto overscroll-contain border-t border-stroke-soft bg-surface-subtle p-[8px_12px]">
                {teamTasks.map((task: TeamTask) => {
                  const role = typeof task.role === 'string' ? task.role.trim() : ''
                  const status = teamTaskStatusLabel(task.status, t)
                  return (
                    <div
                      key={task.id || task.taskName}
                      className="grid min-w-0 gap-[4px] rounded-sm border border-stroke-soft bg-panel p-[7px_8px] text-[12px]"
                    >
                      <div className="flex min-w-0 items-start gap-[6px]">
                        <span
                          aria-hidden="true"
                          className={`team-task-dot mt-[5px] h-[6px] w-[6px] shrink-0 rounded-full ${
                            task.status === 'running'
                              ? 'bg-star [animation:agent-status-pulse_1.6s_ease-in-out_infinite]'
                              : task.status === 'completed'
                                ? 'bg-success'
                                : task.status === 'failed' || task.status === 'interrupted'
                                  ? 'bg-danger'
                                  : task.status === 'blocked'
                                    ? 'bg-warning-strong'
                                    : 'bg-stroke-strong'
                          }`}
                        />
                        <span className="grid min-w-0 gap-[1px]">
                          <strong className="font-medium break-words">
                            {role
                              ? t('chat:agentRunActivity.teamTaskStatus', {
                                  name: task.taskName,
                                  role,
                                  status,
                                })
                              : t('chat:agentRunActivity.teamTaskStatusWithoutRole', {
                                  name: task.taskName,
                                  status,
                                })}
                          </strong>
                          <small className="text-[12px] text-content-muted">{status}</small>
                        </span>
                      </div>
                      {task.error && (
                        <small className="text-[12px] break-words text-danger">{task.error}</small>
                      )}
                      {task.output && (
                        <small className="text-[12px] break-words text-content-muted">
                          {task.output}
                        </small>
                      )}
                    </div>
                  )
                })}
                {(team.blockers?.length ?? 0) > 0 && (
                  <small className="text-[12px] text-warning-strong">
                    {t('chat:agentRunActivity.teamBlocked', { count: team.blockers?.length ?? 0 })}
                  </small>
                )}
                {(team.conflicts?.length ?? 0) > 0 && (
                  <small className="text-[12px] text-danger">
                    {t('chat:agentRunActivity.teamConflicts', {
                      count: team.conflicts?.length ?? 0,
                    })}
                  </small>
                )}
                {team.scriptPath && (
                  <small className="font-mono text-[12px] break-all text-content-muted">
                    {t('chat:agentRunActivity.teamScript', { path: team.scriptPath })}
                  </small>
                )}
                {teamCommunications.length > 0 && (
                  <div className="grid gap-[3px] border-t border-stroke-soft pt-[6px]">
                    <small className="text-[12px] text-content-muted">
                      {t('chat:agentRunActivity.teamCommunicationCount', {
                        count: teamCommunications.length,
                      })}
                    </small>
                    {teamCommunications.slice(-6).map((communication: TeamCommunication) => (
                      <div
                        key={communication.id}
                        className="grid min-w-0 gap-[1px] rounded-xs border-l-2 border-stroke-strong bg-panel p-[4px_8px]"
                      >
                        <strong className="font-mono text-[12px] font-medium break-all">
                          {communication.fromTaskName || communication.fromAgentId || 'lead'} -&gt;{' '}
                          {communication.toTaskName || communication.toAgentId}
                        </strong>
                        <small className="text-[12px] break-words text-content-secondary">
                          {communication.message}
                        </small>
                      </div>
                    ))}
                  </div>
                )}
                {team.summary?.text && (
                  <>
                    <small className="text-[12px] text-content-muted">
                      {t('chat:agentRunActivity.teamSummary')}
                    </small>
                    {team.summary.text && (
                      <p className="m-0 line-clamp-4 text-[12px] leading-[1.45] text-content-secondary">
                        {team.summary.text}
                      </p>
                    )}
                  </>
                )}
              </div>
            </details>
          )}
          {activities.length > 0 && (
            // 运行中与历史共用一个工具组：默认展开；运行中时列表跟随最新一行滚动。
            <details className="agent-run-history min-w-0" open>
              <summary
                className={`flex min-h-[34px] min-w-0 items-center gap-[8px] bg-surface-subtle px-[12px] py-[4px] text-[12px] text-content-secondary ${SUMMARY_BASE_CLASS} hover:bg-surface-hover max-[650px]:min-h-11 pointer-coarse:min-h-11`}
                data-tool-group-summary
              >
                <Activity className="size-[14px] flex-none text-content-muted" aria-hidden="true" />
                <span className="flex min-w-0 flex-1 items-center gap-[6px] truncate tabular-nums">
                  {groupParts.map((part, index) => (
                    <span
                      key={index}
                      className={index === 0 ? 'font-semibold text-content' : undefined}
                    >
                      {index > 0 && (
                        <span aria-hidden="true" className="mr-[6px] text-content-muted">
                          ·
                        </span>
                      )}
                      {part}
                    </span>
                  ))}
                </span>
                <RunDurationLabel
                  startedAt={startedAt}
                  finishedAt={finishedAt}
                  streaming={streaming}
                />
                <ChevronRight className={DISCLOSURE_CLASS} aria-hidden="true" />
              </summary>
              <div
                ref={liveFeedRef}
                className={`agent-run-feed focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus flex max-h-[184px] flex-col overflow-y-auto [overscroll-behavior:contain] border-t border-stroke-soft [&>*+*]:border-t [&>*+*]:border-stroke-soft ${streaming ? 'live' : 'completed'}`}
                aria-label={t('chat:agentRunActivity.toolActivityAriaLabel')}
                tabIndex={activities.length > 3 ? 0 : undefined}
              >
                {streaming ? (
                  <Suspense fallback={activityCards}>
                    <AnimatedList>{activityCards}</AnimatedList>
                  </Suspense>
                ) : (
                  <Suspense fallback={activityCards}>{activityCards}</Suspense>
                )}
              </div>
            </details>
          )}
        </div>
      )}
      {streaming && !compact && computerUseTarget ? (
        <ComputerUseLiveMirror
          fallbackImage={computerUseTarget.previewImage}
          streaming={streaming}
          target={computerUseTarget}
        />
      ) : null}
    </section>
  )
}

function agentRunActivityPropsEqual(prev: AgentRunActivityProps, next: AgentRunActivityProps) {
  return (
    prev.streaming === next.streaming &&
    prev.text === next.text &&
    prev.thinkingText === next.thinkingText &&
    prev.currentActivity === next.currentActivity &&
    prev.team === next.team &&
    prev.activityFeed === next.activityFeed &&
    prev.compaction === next.compaction &&
    prev.error === next.error &&
    prev.stopped === next.stopped &&
    prev.notice === next.notice &&
    prev.startedAt === next.startedAt &&
    prev.lastActivityAt === next.lastActivityAt &&
    prev.finishedAt === next.finishedAt &&
    prev.compact === next.compact &&
    prev.tools === next.tools
  )
}

export default memo(AgentRunActivity, agentRunActivityPropsEqual)
