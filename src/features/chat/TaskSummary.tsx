import { useBackLayer } from '@/hooks/use-back-layer'
import { summarizeProjectDiff } from './project-diff-summary'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Activity, FileDiff, Files, Link2, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetTitle } from '@/components/ui/sheet'
import { useTerminals, useTerminalService } from '@/features/terminal/public'
import { chatApi } from './chat-api'
import { useContextPagesStore } from './session-context-pages-store'
import type { SessionContextTab } from './session-context-pages'
import type { TaskSource } from './task-sources'

export function TaskSummary({
  sessionId,
  cwd,
  taskName,
  sources,
  mobile,
  streaming,
}: {
  sessionId: string
  cwd: string
  taskName: string
  sources: TaskSource[]
  mobile: boolean
  streaming: boolean
}) {
  const { t } = useI18n()
  const records = useTerminals(sessionId)
  const service = useTerminalService()
  const [limit, setLimit] = useState(false)
  const area = useRef<HTMLDivElement>(null)
  const openedContent = useRef(false)
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const vcsAvailable = runtimeFeatureAvailable(capabilities, 'vcs')
  const changes = useQuery({
    enabled: vcsAvailable,
    queryKey: ['task-project-changes', sessionId, cwd, streaming],
    queryFn: ({ signal }) => chatApi.getVcsChanges(sessionId, { signal }),
  })
  const totals = useMemo(
    () =>
      summarizeProjectDiff(
        changes.data?.diff || '',
        changes.data?.files.map((file) => file.path) || [],
        changes.data?.diffTruncated,
      ),
    [changes.data],
  )
  const close = () =>
    useContextPagesStore
      .getState()
      .update(sessionId, 'new', { type: 'workspace', summaryOpen: false })
  const restore = () => document.getElementById(`task-${sessionId}-summary-toggle`)?.focus()
  useEffect(() => {
    if (!mobile) area.current?.focus()
  }, [mobile])
  const open = (kind: SessionContextTab, id = '') => {
    const store = useContextPagesStore.getState()
    const result = store.update(sessionId, kind, { type: 'open', kind })
    if (result === 'limit') {
      setLimit(true)
      return
    }
    openedContent.current = true
    const pageId = useContextPagesStore.getState().sessions[sessionId]?.activeId
    if (id && pageId)
      store.update(sessionId, kind, {
        type: 'view',
        id: pageId,
        patch:
          kind === 'sources'
            ? { selectedSourceId: id, query: '', filter: '' }
            : { expandedProcesses: [id], filter: '' },
      })
  }
  useBackLayer(!mobile, 300, () => {
    close()
    restore()
  })
  const body = (
    <>
      {limit && (
        <p role="alert" className="text-xs text-destructive">
          {t('chat:sessionContext.pageLimit')}
        </p>
      )}
      <div className="flex min-w-0 items-start gap-2 border-b border-border pb-3">
        <div className="min-w-0 flex-1">
          <p className="truncate text-xs text-content-muted" title={cwd}>
            {cwd.split(/[\\/]/).filter(Boolean).at(-1) || cwd}
          </p>
          <p className="mt-1 truncate text-xs text-content-muted">{taskName}</p>
        </div>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t('chat:workbench.closeSummary')}
          onClick={() => {
            close()
            restore()
          }}
        >
          <X size={16} />
        </Button>
      </div>
      <section className="space-y-2 border-b border-border py-3">
        <Button
          variant="ghost"
          className="h-auto w-full justify-start whitespace-normal text-left"
          disabled={!vcsAvailable}
          onClick={() => open('changes')}
        >
          <FileDiff size={16} />
          <span className="min-w-0 flex-1">{t('chat:workbench.projectChanges')}</span>
          <span className="text-xs text-content-muted">
            {!vcsAvailable
              ? t('chat:workbench.unavailable')
              : changes.isPending
                ? '…'
                : changes.isError || changes.data?.error
                  ? t('chat:workbench.unavailable')
                  : changes.data?.isRepo
                    ? changes.data.files.length
                    : '—'}
          </span>
        </Button>
        {(changes.isError || changes.data?.error) && (
          <p role="alert" className="px-3 text-xs text-destructive">
            {t('chat:workbench.changesFailed')}
          </p>
        )}
        <Button variant="ghost" className="w-full justify-start" onClick={() => open('files')}>
          <Files size={16} />
          {t('chat:workbench.taskSnapshots')}
        </Button>
      </section>
      {changes.data?.isRepo && !changes.isError && !changes.data.error && (
        <p className="px-3 text-xs text-content-muted">
          {totals.partial ? t('chat:workbench.partialCounts') : ''} +{totals.added} −
          {totals.removed}
        </p>
      )}
      <section className="space-y-1 border-b border-border py-3">
        <Button
          variant="ghost"
          className="w-full justify-between"
          onClick={() => open('processes')}
        >
          <span>{t('terminal:workspace.processes')}</span>
          <span className="text-content-muted">{records.length}</span>
        </Button>
        {!service && (
          <p className="px-3 text-xs text-content-muted">{t('terminal:workspace.nativeOnly')}</p>
        )}
        {records.slice(0, 3).map((record) => (
          <Button
            key={record.id}
            variant="ghost"
            className="h-auto w-full min-w-0 justify-start py-2"
            onClick={() => open('processes', record.id)}
          >
            <Activity size={15} />
            <span className="min-w-0 flex-1 truncate text-left">{record.profileId}</span>
            <span className="text-xs text-content-muted">
              {record.status === 'running'
                ? t('terminal:workspace.running')
                : record.status === 'exited'
                  ? t('terminal:workspace.exited')
                  : t('chat:workbench.otherState')}
            </span>
          </Button>
        ))}
      </section>
      <section className="space-y-1 pt-3">
        <Button variant="ghost" className="w-full justify-between" onClick={() => open('sources')}>
          <span>{t('chat:workbench.sources')}</span>
          <span className="text-content-muted">{sources.length}</span>
        </Button>
        {sources.slice(0, 3).map((source) => (
          <Button
            key={source.id}
            variant="ghost"
            className="w-full min-w-0 justify-start"
            onClick={() => open('sources', source.id)}
          >
            <Link2 size={15} />
            <span className="truncate">{source.name || t('chat:chatMessage.fileAttachment')}</span>
          </Button>
        ))}
        <Button
          variant="ghost"
          className="w-full justify-start text-content-muted"
          onClick={() => open('sources')}
        >
          {t('chat:workbench.viewAll')}
        </Button>
      </section>
    </>
  )
  if (mobile)
    return (
      <Sheet
        open
        onOpenChange={(value) => {
          if (!value) close()
        }}
      >
        <SheetContent
          side="bottom"
          showCloseButton={false}
          aria-describedby={undefined}
          className="max-h-[80dvh] overflow-y-auto rounded-t-2xl p-4 pb-[max(16px,var(--pisper-safe-area-bottom))] [&_button]:font-normal"
          style={{
            left: 'calc(44px + var(--pisper-safe-area-left, 0px))',
            right: 'var(--pisper-safe-area-right, 0px)',
          }}
          onCloseAutoFocus={(event) => {
            event.preventDefault()
            if (!openedContent.current) restore()
          }}
        >
          <SheetTitle className="sr-only">{t('chat:workbench.summary')}</SheetTitle>
          {body}
        </SheetContent>
      </Sheet>
    )
  return (
    <div
      ref={area}
      tabIndex={-1}
      id={`task-${sessionId}-summary`}
      role="region"
      aria-label={t('chat:workbench.summary')}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault()
          event.stopPropagation()
          close()
          restore()
        }
      }}
      className="absolute right-3 top-14 z-20 max-h-[calc(100%-68px)] w-80 max-w-[calc(100%-24px)] overflow-y-auto rounded-2xl border border-border bg-popover p-3 text-sm shadow-[0_4px_16px_#0000000d] [&_button]:font-normal"
    >
      {body}
    </div>
  )
}
