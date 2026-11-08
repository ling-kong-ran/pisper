// 会话顶栏的“运行中”芯片：这个会话此刻还在跑的东西——代理正在执行的命令与工具、子代理、
// 本任务的终端与后台进程。原来是检查器里的一个标签，现在不占面板：没东西在跑时不显示，
// 点开是弹层，终端 / 进程一行可跳到检查器“终端 › 进程”看输出与停止。
// 数据都来自会话已有的实时状态，不另开轮询；只有 Pisper 自己追踪的才列得出来，
// 命令自己放到后台（nohup、&）的进程不在其中，弹层页脚写明。
import { useContext, useEffect, useState, type ReactNode } from 'react'
import { Bot, ChevronRight, TerminalSquare, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTitle, PopoverTrigger } from '@/components/ui/popover'
import { useTerminals } from '@/features/terminal/public'
import { cn } from '@/lib/utils'
import type { EntityRecord } from '@/types/chat'
import { formatTrayClock } from './focus-session-composer-bits'
import { runDurationMs } from './run-activity'
import { useSessionInspectorStore } from './session-inspector-store'
import { SessionInspectorContext } from './session-workspace-context'

const ACTIVE_AGENT_STATUSES = new Set(['queued', 'starting', 'running'])
const ACTIVE_TERMINAL_STATUSES = new Set(['starting', 'running', 'stopping'])

type RunningRow = {
  key: string
  kind: 'tool' | 'agent' | 'terminal'
  icon: ReactNode
  name: string
  detail: string
  startedAt?: string | null
  status: string
}

function text(value: unknown) {
  return typeof value === 'string' ? value.trim() : ''
}

// 同一个工具 / 子代理会以多条活动出现，按 id 去重并保留最新的一条。
function latestById(
  activity: readonly EntityRecord[],
  type: string,
  read: (entry: EntityRecord) => EntityRecord | null,
) {
  const byId = new Map<string, EntityRecord>()
  for (const entry of activity) {
    if (entry?.type !== type) continue
    const record = read(entry)
    if (record) byId.set(String(record.id || byId.size), record)
  }
  return [...byId.values()]
}

// 每秒刷新只留在这段文字内，列表不随计时器重渲染。
function Elapsed({ startedAt }: { startedAt?: string | null }) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1_000)
    return () => window.clearInterval(timer)
  }, [])
  const elapsed = runDurationMs(startedAt, null, now)
  if (!elapsed) return null
  return (
    <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
      {formatTrayClock(elapsed)}
    </span>
  )
}

function useRunningRows(
  sessionId: string,
  streaming: boolean,
  activity: readonly EntityRecord[],
): RunningRow[] {
  const { t } = useI18n()
  const terminals = useTerminals(sessionId)
  return [
    // 运行结束后活动里可能还留着没结算的 running：会话没在跑时工具不算。
    ...(streaming
      ? latestById(activity, 'tool', (entry) => entry)
          .filter((tool) => tool.status === 'running')
          .map((tool) => {
            const args = (tool.args || {}) as EntityRecord
            return {
              key: `tool:${String(tool.id)}`,
              kind: 'tool' as const,
              icon: <Wrench size={14} aria-hidden="true" />,
              name: text(tool.name) || t('board:inspector.runningTool'),
              detail: text(args.command) || text(args.path) || text(args.file_path),
              startedAt: text(tool.startedAt),
              status: t('board:inspector.runningStatus'),
            }
          })
      : []),
    ...latestById(activity, 'agent', (entry) =>
      entry.agent && typeof entry.agent === 'object' ? (entry.agent as EntityRecord) : null,
    )
      .filter((agent) => ACTIVE_AGENT_STATUSES.has(String(agent.status)))
      .map((agent) => ({
        key: `agent:${String(agent.id)}`,
        kind: 'agent' as const,
        icon: <Bot size={14} aria-hidden="true" />,
        name: text(agent.title) || text(agent.taskName) || t('board:inspector.runningAgent'),
        detail: text(agent.message),
        startedAt: text(agent.startedAt),
        status:
          agent.status === 'queued'
            ? t('board:inspector.runningQueued')
            : t('board:inspector.runningStatus'),
      })),
    ...terminals
      .filter((record) => ACTIVE_TERMINAL_STATUSES.has(record.status))
      .map((record) => ({
        key: `terminal:${record.id}`,
        kind: 'terminal' as const,
        icon: <TerminalSquare size={14} aria-hidden="true" />,
        name: record.profileId || t('board:inspector.terminal'),
        detail: record.cwd || '',
        status:
          record.status === 'stopping'
            ? t('terminal:workspace.stopping')
            : t('board:inspector.runningStatus'),
      })),
  ]
}

function RowBody({ row }: { row: RunningRow }) {
  return (
    <>
      <span className="mt-0.5 text-content-secondary">{row.icon}</span>
      <span className="grid min-w-0 gap-0.5">
        <strong className="truncate text-[13px] font-medium text-content">{row.name}</strong>
        {row.detail && (
          <span className="min-w-0 truncate font-mono text-content-muted" title={row.detail}>
            {row.detail}
          </span>
        )}
      </span>
      <span className="flex flex-none items-center gap-2">
        <span className="text-content-muted">{row.status}</span>
        <Elapsed startedAt={row.startedAt} />
      </span>
    </>
  )
}

const ROW_CLASS =
  'grid min-h-9 w-full min-w-0 grid-cols-[14px_minmax(0,1fr)_auto] items-start gap-2 rounded-sm px-2 py-1.5 text-left text-xs pointer-coarse:min-h-11'

export function SessionRunningChip({
  sessionId,
  streaming,
  activity,
  phoneButtonClass,
}: {
  sessionId: string
  streaming: boolean
  activity: readonly EntityRecord[]
  // 手机顶栏：和顶栏其他图标键同一种 44 的圆键，数字是角标。
  phoneButtonClass?: string
}) {
  const { t } = useI18n()
  const inspector = useContext(SessionInspectorContext)
  const [open, setOpen] = useState(false)
  const rows = useRunningRows(sessionId, streaming, activity)
  if (!rows.length) return null
  const label = t('board:inspector.runningCount', { count: rows.length })
  // 终端 / 进程：到检查器“终端 › 进程”看输出、停止。
  const openProcesses = () => {
    setOpen(false)
    useSessionInspectorStore.getState().setTerminalView('processes')
    inspector?.show('terminal')
  }
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        {phoneButtonClass ? (
          <button
            type="button"
            className={phoneButtonClass}
            aria-label={label}
            data-session-running={rows.length}
          >
            <StatusGlyph status="run" className="size-5" />
            <span
              aria-hidden="true"
              className="absolute top-1.5 right-1 grid h-4 min-w-4 place-items-center rounded-full bg-content px-1 text-[10px] leading-none font-semibold text-panel tabular-nums"
            >
              {rows.length > 99 ? '99+' : rows.length}
            </span>
          </button>
        ) : (
          <Button
            type="button"
            variant="ghost"
            className={cn(
              'h-8 flex-none gap-1.5 px-2 text-[13px] font-normal text-content-secondary tabular-nums hover:text-content pointer-coarse:h-11',
              open && 'bg-surface-hover text-content',
            )}
            aria-label={label}
            title={label}
            data-session-running={rows.length}
          >
            <StatusGlyph status="run" className="size-3.5" />
            <span aria-hidden="true">{label}</span>
          </Button>
        )}
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-[380px] gap-1.5 p-1.5"
        aria-label={t('board:inspector.running')}
        data-session-running-list
      >
        <PopoverTitle className="px-2 pt-1 text-xs font-medium text-content-muted">
          {t('board:inspector.running')}
        </PopoverTitle>
        <ul className="m-0 flex list-none flex-col gap-px p-0">
          {rows.map((row) => (
            <li key={row.key} data-running-item={row.kind}>
              {row.kind === 'terminal' && inspector ? (
                <button
                  type="button"
                  className={cn(
                    ROW_CLASS,
                    'group cursor-pointer hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
                  )}
                  aria-label={t('board:inspector.runningOpenProcess', { name: row.name })}
                  onClick={openProcesses}
                >
                  <RowBody row={row} />
                </button>
              ) : (
                <div className={ROW_CLASS}>
                  <RowBody row={row} />
                </div>
              )}
            </li>
          ))}
        </ul>
        {rows.some((row) => row.kind === 'terminal') && inspector && (
          <button
            type="button"
            className="flex h-8 w-full cursor-pointer items-center justify-between rounded-sm px-2 text-[13px] text-content-secondary hover:bg-surface-hover hover:text-content pointer-coarse:h-11"
            onClick={openProcesses}
          >
            {t('board:inspector.runningAllProcesses')}
            <ChevronRight size={14} aria-hidden="true" />
          </button>
        )}
        <p className="m-0 border-t border-stroke-soft px-2 pt-2 pb-1 text-xs leading-relaxed text-content-faint">
          {t('board:inspector.runningHint')}
        </p>
      </PopoverContent>
    </Popover>
  )
}
