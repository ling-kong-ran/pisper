import { useLayoutEffect, useRef, useState } from 'react'
import { ChevronRight, Square, TerminalSquare } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { PageState } from '@/components/app/PageState'
import { useTerminalService, useTerminals } from './terminal-context'
import type { TerminalRecord } from './terminal-registry'

type ProcessViewState = {
  filter: string
  expandedProcesses: string[]
  processFollow: Record<string, boolean>
  processScroll: Record<string, number>
}
function ProcessLog({
  record,
  follow,
  initialTop,
  onFollow,
  onScroll,
}: {
  record: TerminalRecord
  follow: boolean
  initialTop: number
  onFollow: (value: boolean) => void
  onScroll: (top: number) => void
}) {
  const { t } = useI18n()
  const area = useRef<HTMLPreElement>(null)
  const initial = useRef(initialTop)
  useLayoutEffect(() => {
    if (area.current) area.current.scrollTop = initial.current
  }, [])
  useLayoutEffect(() => {
    if (follow && area.current) area.current.scrollTop = area.current.scrollHeight
  }, [record.output, follow])
  return (
    <div className="border-t border-border p-3">
      <label className="mb-2 flex items-center gap-2 text-xs text-content-muted">
        <input
          type="checkbox"
          checked={follow}
          onChange={(event) => onFollow(event.target.checked)}
        />
        {t('terminal:workspace.follow')}
      </label>
      {record.truncated && (
        <p className="mb-2 text-xs text-content-muted">{t('terminal:workspace.truncated')}</p>
      )}
      <pre
        ref={area}
        tabIndex={0}
        onWheel={(event) => {
          if (event.deltaY < 0) onFollow(false)
        }}
        onScroll={(event) => {
          const el = event.currentTarget
          if (el.scrollTop + el.clientHeight < el.scrollHeight - 4) onFollow(false)
          onScroll(el.scrollTop)
        }}
        className="max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-md bg-surface-subtle p-3 font-mono text-xs"
      >
        {record.output || t('terminal:workspace.noOutput')}
      </pre>
    </div>
  )
}

export function TaskProcessesPane({
  sessionId,
  onOpenTerminal,
  view,
  onViewChange,
}: {
  sessionId: string
  onOpenTerminal: (id: string) => void
  view: ProcessViewState
  onViewChange: (patch: Partial<ProcessViewState>) => void
}) {
  const { t } = useI18n()
  const service = useTerminalService()
  const records = useTerminals(sessionId)
  const filter = view.filter || 'all'
  const expanded = new Set(view.expandedProcesses)
  const [error, setError] = useState('')
  if (!service)
    return (
      <PageState
        kind="empty"
        title={t('terminal:workspace.unavailable')}
        description={t('terminal:workspace.nativeOnly')}
      />
    )
  const statuses = {
    starting: t('terminal:workspace.starting'),
    running: t('terminal:workspace.running'),
    stopping: t('terminal:workspace.stopping'),
    exited: t('terminal:workspace.exited'),
    error: t('terminal:workspace.error'),
  }
  const visible = records.filter((record) => filter === 'all' || record.status === filter)
  return (
    <section className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-3 border-b border-border p-3">
        <span className="flex-1 text-xs text-content-muted">
          {t('terminal:workspace.ownedProcesses')}
        </span>
        <select
          aria-label={t('terminal:workspace.filter')}
          value={filter}
          onChange={(event) => onViewChange({ filter: event.target.value })}
          className="rounded-md border border-border bg-background px-2 py-1 text-sm"
        >
          <option value="all">{t('terminal:workspace.all')}</option>
          {Object.entries(statuses).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </div>
      {error && (
        <p role="alert" className="p-3 text-sm text-danger">
          {error}
        </p>
      )}
      <div className="min-h-0 flex-1 overflow-y-auto p-3">
        {!visible.length ? (
          <PageState
            kind="empty"
            title={
              records.length ? t('terminal:workspace.noMatch') : t('terminal:workspace.noProcesses')
            }
          />
        ) : (
          visible.map((record) => (
            <article
              className="mb-2 overflow-hidden rounded-lg border border-border"
              key={record.id}
            >
              <div className="flex min-w-0 items-center gap-2 p-2">
                <Button
                  variant="ghost"
                  size="icon"
                  aria-expanded={expanded.has(record.id)}
                  aria-label={t('terminal:workspace.logs')}
                  onClick={() =>
                    onViewChange({
                      expandedProcesses: expanded.has(record.id)
                        ? view.expandedProcesses.filter((id) => id !== record.id)
                        : [...view.expandedProcesses, record.id],
                    })
                  }
                >
                  <ChevronRight className={expanded.has(record.id) ? 'rotate-90' : ''} />
                </Button>
                <div className="min-w-0 flex-1">
                  <strong className="block truncate text-sm font-medium">{record.profileId}</strong>
                  <span className="block truncate text-xs text-content-muted" title={record.cwd}>
                    {record.cwd}
                  </span>
                </div>
                <span className="text-xs text-content-muted">
                  {statuses[record.status] || t('terminal:workspace.unknown')}
                </span>
                <Button
                  variant="ghost"
                  size="icon"
                  aria-label={t('terminal:workspace.open')}
                  onClick={() => onOpenTerminal(record.id)}
                >
                  <TerminalSquare />
                </Button>
                {record.status !== 'exited' && (
                  <Button
                    variant="ghost"
                    size="icon"
                    disabled={record.status === 'stopping'}
                    aria-label={t('terminal:workspace.stop')}
                    onClick={() => {
                      setError('')
                      void service.registry
                        .stop(record.id)
                        .catch((caught) => setError(String(caught)))
                    }}
                  >
                    <Square className="size-4" />
                  </Button>
                )}
              </div>
              {record.error && <p className="px-3 pb-2 text-xs text-danger">{record.error}</p>}
              {expanded.has(record.id) && (
                <ProcessLog
                  record={record}
                  follow={view.processFollow[record.id] !== false}
                  initialTop={view.processScroll[record.id] || 0}
                  onFollow={(value) =>
                    onViewChange({ processFollow: { ...view.processFollow, [record.id]: value } })
                  }
                  onScroll={(top) =>
                    onViewChange({ processScroll: { ...view.processScroll, [record.id]: top } })
                  }
                />
              )}
            </article>
          ))
        )}
      </div>
    </section>
  )
}
