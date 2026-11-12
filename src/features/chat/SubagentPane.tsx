// 检查器的“子代理”标签：只读看会话派出的一个子代理的完整过程。
// 页头是一行标题（状态图标 + 名字，名字本身就是切换下拉）、右侧用时与停止；下面一行灰字写它在做什么、模型与工具次数；
// 再下面是主代理交给它的任务（引用样式，默认两行，超出时才给“展开”）。
// 正文复用会话转录，和普通会话一样显示它说的话、调的工具和命令输出，没有输入框也没有重试。
// 在跑时 2.5 秒拉一次，结束后停；子代理在保存过程之前派出时只给出它交回的结果。
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Bot, CornerDownLeft, Square } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import type { EntityRecord } from '@/types/chat'
import { ChatRequestNotice } from './ChatRequestNotice'
import { chatErrorMessage } from './chat-errors'
import { FocusTranscript } from './FocusTranscript'
import {
  fetchAgentTranscript,
  streamAgentTranscript,
  isActiveAgent,
  stopSessionAgent,
  useSessionAgents,
  type AgentTranscript,
  type SessionAgent,
} from './session-agents'
import { TranscriptMetaContext } from './session-workspace-context'
import {
  SUBAGENT_STATUS_GLYPH,
  subagentActivityText,
  subagentElapsed,
  subagentStatusText,
} from './subagent-presentation'
import { useSubagentViewStore } from './subagent-view-store'

const ACTIVE_POLL_MS = 2_500
// 推送断了（子代理还在跑）时隔多久重连。
const RECONNECT_MS = 1_500

function AgentGlyph({ agent }: { agent: SessionAgent }) {
  const glyph = SUBAGENT_STATUS_GLYPH[agent.status]
  return (
    <StatusGlyph
      status={glyph.kind}
      spin={glyph.kind === 'run'}
      className={cn('size-[15px] flex-none', glyph.color)}
    />
  )
}

function useNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return undefined
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [active])
  return now
}

function clockTime(value: string | null, language: string) {
  const time = value ? Date.parse(value) : Number.NaN
  return Number.isFinite(time)
    ? new Date(time).toLocaleTimeString(language, { hour: '2-digit', minute: '2-digit' })
    : ''
}

const NO_ACTIVITY: EntityRecord[] = []

// 收起的任务是否真的被截断：只有截断了才给“展开”，短任务不放一个没用的按钮。
// 宽度一变（拖动检查器、旋转手机）行数就变，所以在收起状态下跟着元素尺寸重新量。
function useClampOverflow(text: string, clamped: boolean) {
  const ref = useRef<HTMLParagraphElement>(null)
  const [overflows, setOverflows] = useState(false)
  useLayoutEffect(() => {
    const element = ref.current
    if (!element || !clamped) return undefined
    // 留 1px 余量：小数行高在部分缩放比例下会让 scrollHeight 比 clientHeight 多出零点几像素。
    const measure = () => setOverflows(element.scrollHeight > element.clientHeight + 1)
    measure()
    if (typeof ResizeObserver === 'undefined') return undefined
    const observer = new ResizeObserver(measure)
    observer.observe(element)
    return () => observer.disconnect()
  }, [text, clamped])
  return [ref, overflows] as const
}

// 换一个子代理时由调用方按 id 重新挂载，展开状态随之回到收起。
function SubagentTask({ text }: { text: string }) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [ref, overflows] = useClampOverflow(text, !open)
  const textId = useId()
  const labelId = useId()
  return (
    // 引用样式：左侧一道细线，不再是页头里再套一张灰卡片；任务是背景信息，字色比正文浅一级。
    <div
      role="group"
      aria-labelledby={labelId}
      className="mx-1.5 mt-2 border-l-2 border-stroke pl-2.5"
    >
      <p id={labelId} className="m-0 text-[11px] leading-4 font-medium text-content-muted">
        {t('board:subagent.task')}
      </p>
      <p
        ref={ref}
        id={textId}
        className={cn(
          'm-0 mt-0.5 text-[13px] leading-5 break-words whitespace-pre-wrap text-content-secondary',
          // 收起时只截两行、不滚动（overflow-auto 会盖掉截断，露出半行）；展开后过长才在块内滚动。
          open ? 'max-h-60 overflow-y-auto overscroll-contain' : 'line-clamp-2',
        )}
      >
        {text}
      </p>
      {(overflows || open) && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          aria-expanded={open}
          aria-controls={textId}
          // 按钮自带的左内边距让文字往里缩，负外边距把“展开”两个字对齐到任务正文的左缘；触屏照常 44px。
          className="-ml-2.5 text-xs"
          onClick={() => setOpen((value) => !value)}
        >
          {open ? t('board:subagent.taskCollapse') : t('board:subagent.taskExpand')}
        </Button>
      )}
    </div>
  )
}

export function SubagentPane({ sessionId, sheet = false }: { sessionId: string; sheet?: boolean }) {
  const { t, language } = useI18n()
  const selectedId = useSubagentViewStore((state) => state.selected[sessionId] ?? '')
  const select = useSubagentViewStore((state) => state.select)
  const { agents: all, reload } = useSessionAgents(sessionId, true)
  // 新 Runtime 推送子代理的过程（正在写的字也在里面）；旧的只能轮询。
  const pushed = useRuntimeCapabilitiesStore(
    (state) => state.capabilities.features.agentTranscriptEvents,
  )
  // 后台命令没有对话可看，不在切换列表里。
  const agents = useMemo(() => all.filter((agent) => agent.kind !== 'command'), [all])
  const agent = agents.find((item) => item.id === selectedId)
  const active = Boolean(agent && isActiveAgent(agent))
  const now = useNow(active)
  const [transcript, setTranscript] = useState<AgentTranscript | null>(null)
  const [error, setError] = useState('')
  const [stopping, setStopping] = useState(false)
  const [attempt, setAttempt] = useState(0)
  const agentId = agent?.id ?? ''
  const status = agent?.status ?? ''

  useEffect(() => {
    setTranscript(null)
    setError('')
  }, [agentId])

  // 子代理的状态变了（比如刚跑完）也重拉一次，拿到最后一段回复。
  useEffect(() => {
    if (!agentId) return undefined
    const controller = new AbortController()
    let timer = 0
    if (pushed) {
      let received = false
      const open = async () => {
        let last: AgentTranscript | null = null
        try {
          last = await streamAgentTranscript(
            sessionId,
            agentId,
            (next) => {
              received = true
              setTranscript(next)
              setError('')
            },
            controller.signal,
          )
        } catch (cause) {
          if (controller.signal.aborted) return
          // 一帧都没收到就失败：显示错误（可重试）；收到过的是中途断了，悄悄重连。
          if (!received) {
            setError(chatErrorMessage(cause))
            return
          }
        }
        if (controller.signal.aborted || last?.active === false) return
        timer = window.setTimeout(() => void open(), RECONNECT_MS)
      }
      void open()
      return () => {
        controller.abort()
        window.clearTimeout(timer)
      }
    }
    const load = async () => {
      try {
        const next = await fetchAgentTranscript(sessionId, agentId, controller.signal)
        if (controller.signal.aborted) return
        setTranscript(next)
        setError('')
        if (next.active) timer = window.setTimeout(load, ACTIVE_POLL_MS)
      } catch (cause) {
        if (!controller.signal.aborted) setError(chatErrorMessage(cause))
      }
    }
    void load()
    return () => {
      controller.abort()
      window.clearTimeout(timer)
    }
  }, [agentId, attempt, pushed, sessionId, status])

  const stop = useCallback(async () => {
    if (!agentId) return
    setStopping(true)
    try {
      await stopSessionAgent(sessionId, agentId)
    } catch {
      // 已经结束了也算停下：下一次刷新显示最终状态。
    } finally {
      setStopping(false)
      reload()
    }
  }, [agentId, reload, sessionId])

  const meta = useMemo(() => ({ model: '', name: agent?.taskName ?? '' }), [agent?.taskName])
  const messages = useMemo(
    () => (transcript?.taskShown ? transcript.messages.slice(1) : (transcript?.messages ?? [])),
    [transcript],
  )
  // 还在跑时转录按进行中的回复画，最新一轮读的是“实时活动”而不是消息自带的快照；
  // 只读转录没有实时流，把最后一条回复的活动当作实时活动交进去，否则这一轮的工具组整组不见。
  const liveRun = useMemo(() => {
    const last = messages.at(-1)
    return transcript?.active && last?.role === 'agent' && last.runActivity
      ? (last.runActivity as EntityRecord)
      : null
  }, [messages, transcript?.active])

  if (!agent)
    return (
      <InlineEmptyState
        compact
        icon={Bot}
        title={agents.length ? t('board:subagent.pick') : t('board:subagent.empty')}
      />
    )

  const task = transcript?.task || agent.message
  const details = [
    agent.model.split('/').pop(),
    agent.toolCallCount ? t('board:subagent.toolCalls', { count: agent.toolCallCount }) : '',
    t('board:subagent.startedAt', { time: clockTime(agent.startedAt, language) }),
  ]
    .filter(Boolean)
    .join(' · ')

  let body
  if (error)
    body = (
      <div className="p-3">
        <ChatRequestNotice error={error} onRetry={() => setAttempt((value) => value + 1)} />
      </div>
    )
  else if (!transcript)
    body = (
      <p role="status" className="p-4 text-sm text-content-muted">
        {t('board:subagent.loading')}
      </p>
    )
  else if (!transcript.available)
    // 保存过程之前派出的子代理：只有它交回的结果。
    body = (
      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
        <InlineEmptyState compact icon={Bot} title={t('board:subagent.notRecorded')} />
        {(agent.output || agent.error) && (
          <p className="m-0 text-[13px] leading-relaxed whitespace-pre-wrap text-content">
            {agent.error || agent.output}
          </p>
        )}
      </div>
    )
  else
    body = (
      <TranscriptMetaContext.Provider value={meta}>
        {transcript.omitted > 0 && (
          <p className="m-0 shrink-0 px-3 pt-2 text-xs text-content-muted">
            {t('board:subagent.omitted', { count: transcript.omitted })}
          </p>
        )}
        <FocusTranscript
          sessionId={`${sessionId}:agent:${agent.id}`}
          messages={messages}
          transcriptLoadState="ready"
          emptyState={
            <InlineEmptyState compact icon={Bot} title={t('board:subagent.noMessages')} />
          }
          activityFeed={liveRun?.activityFeed ?? NO_ACTIVITY}
          tools={liveRun?.tools ?? NO_ACTIVITY}
          thinkingText={liveRun?.thinkingText}
          runStartedAt={liveRun?.startedAt ?? null}
          lastActivityAt={liveRun?.lastActivityAt ?? null}
          // 子代理还在跑：按进行中的回复画（跟到底部、工具组写“运行中”）。
          streaming={transcript.active}
          readOnly
          onRetryLastTurn={() => {}}
        />
      </TranscriptMetaContext.Provider>
    )

  const elapsed = subagentElapsed(agent.startedAt, agent.completedAt, now)
  return (
    <section
      aria-label={t('board:subagent.label', { name: agent.taskName })}
      data-subagent-pane={agent.id}
      className="flex h-full min-h-0 min-w-0 flex-1 flex-col [container-type:inline-size]"
    >
      <div className="flex shrink-0 flex-col border-b border-stroke-soft px-2 pt-2 pb-3">
        <div className="flex min-w-0 items-center gap-2">
          {/* 名字就是切换：无边框的下拉触发键，读起来是页头标题而不是一个表单输入框。 */}
          <Select value={agent.id} onValueChange={(id) => select(sessionId, id)}>
            <SelectTrigger
              variant="ghost"
              aria-label={t('board:subagent.switch')}
              className="min-w-0"
            >
              <SelectValue>
                <AgentGlyph agent={agent} />
                <span className="min-w-0 truncate font-semibold text-content">
                  {agent.taskName}
                </span>
              </SelectValue>
            </SelectTrigger>
            <SelectContent align="start">
              {agents.map((item) => (
                <SelectItem key={item.id} value={item.id}>
                  <AgentGlyph agent={item} />
                  <span className="truncate">{item.taskName}</span>
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <span className="ml-auto flex-none text-xs whitespace-nowrap text-content-muted tabular-nums">
            {/* 很窄时只留用时：状态已经由名字前的图标表达，文字留给读屏。 */}
            <span className="@max-[340px]:sr-only">{subagentStatusText(agent.status, t)} · </span>
            {elapsed}
          </span>
          {active && (
            <Button
              type="button"
              variant="secondary"
              size="sm"
              className="flex-none"
              disabled={stopping}
              onClick={() => void stop()}
            >
              <Square aria-hidden="true" />
              {t('board:subagent.stop')}
            </Button>
          )}
        </div>
        <p className="m-0 truncate px-1.5 text-xs leading-5 text-content-muted">
          {active ? `${subagentActivityText(agent, t)} · ${details}` : details}
        </p>
        {task && <SubagentTask key={agent.id} text={task} />}
      </div>
      {/* 正文占满剩下的高度：读取中、出错和没有记录时也一样，页脚始终贴在面板底部。 */}
      <div className="flex min-h-0 flex-1 flex-col">{body}</div>
      <p
        className={cn(
          'm-0 flex shrink-0 items-center gap-1.5 border-t border-stroke-soft px-3 py-1.5 text-xs leading-4 text-content-muted',
          sheet ? 'min-h-11' : 'min-h-9',
        )}
      >
        <CornerDownLeft aria-hidden="true" className="size-3.5 flex-none" />
        {active
          ? t('board:subagent.footerRunning')
          : agent.status === 'completed'
            ? t('board:subagent.footerReturned', {
                time: clockTime(agent.completedAt, language),
              })
            : t('board:subagent.footerEnded')}
      </p>
    </section>
  )
}
