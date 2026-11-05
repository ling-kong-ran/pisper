// 检查器“运行中”标签：这个会话此刻还在跑的东西——代理正在执行的命令与工具、子代理、本任务的终端。
// 中断后想确认“到底还有没有东西在跑”时看这里。数据都来自会话已有的实时状态，不另开轮询；
// 只有 Pisper 自己追踪的才列得出来，命令自己放到后台（nohup、&）的进程不在其中，页脚写明。
import { useEffect, useState, type ReactNode } from 'react'
import { Bot, TerminalSquare, Wrench } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { useTerminals } from '@/features/terminal/public'
import type { EntityRecord } from '@/types/chat'
import { formatTrayClock } from './focus-session-composer-bits'
import { runDurationMs } from './run-activity'

const ACTIVE_AGENT_STATUSES = new Set(['queued', 'starting', 'running'])
const ACTIVE_TERMINAL_STATUSES = new Set(['starting', 'running', 'stopping'])

type RunningRow = {
  key: string
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

export function runningCount(activity: readonly EntityRecord[], streaming: boolean) {
  const tools = streaming
    ? latestById(activity, 'tool', (entry) => entry).filter((tool) => tool.status === 'running')
    : []
  const agents = latestById(activity, 'agent', (entry) =>
    entry.agent && typeof entry.agent === 'object' ? (entry.agent as EntityRecord) : null,
  ).filter((agent) => ACTIVE_AGENT_STATUSES.has(String(agent.status)))
  return tools.length + agents.length
}

export function InspectorRunning({
  sessionId,
  streaming,
  activity,
}: {
  sessionId: string
  streaming: boolean
  activity: readonly EntityRecord[]
}) {
  const { t } = useI18n()
  const terminals = useTerminals(sessionId)
  const rows: RunningRow[] = [
    // 运行结束后活动里可能还留着没结算的 running：会话没在跑时工具不算。
    ...(streaming
      ? latestById(activity, 'tool', (entry) => entry)
          .filter((tool) => tool.status === 'running')
          .map((tool) => {
            const args = (tool.args || {}) as EntityRecord
            return {
              key: `tool:${String(tool.id)}`,
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
        icon: <TerminalSquare size={14} aria-hidden="true" />,
        name: record.profileId || t('board:inspector.terminal'),
        detail: record.cwd || '',
        status:
          record.status === 'stopping'
            ? t('terminal:workspace.stopping')
            : t('board:inspector.runningStatus'),
      })),
  ]
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto p-3" data-inspector-running>
      {rows.length ? (
        <ul className="m-0 flex list-none flex-col gap-px p-0">
          {rows.map((row) => (
            <li
              key={row.key}
              className="grid min-w-0 grid-cols-[14px_minmax(0,1fr)_auto] items-start gap-2 py-1.5 text-xs"
              data-running-item={row.key.split(':')[0]}
            >
              <span className="mt-0.5 text-content-secondary">{row.icon}</span>
              <span className="grid min-w-0 gap-0.5">
                <strong className="truncate text-[13px] font-medium text-content">
                  {row.name}
                </strong>
                {row.detail && (
                  <span
                    className="min-w-0 truncate font-mono text-content-muted"
                    title={row.detail}
                  >
                    {row.detail}
                  </span>
                )}
              </span>
              <span className="flex flex-none items-center gap-2">
                <span className="text-content-muted">{row.status}</span>
                <Elapsed startedAt={row.startedAt} />
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <InlineEmptyState compact title={t('board:inspector.runningEmpty')} />
      )}
      <p className="mt-3 text-xs leading-relaxed text-content-faint">
        {t('board:inspector.runningHint')}
      </p>
    </div>
  )
}
