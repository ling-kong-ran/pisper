import { useBackLayer } from '@/hooks/use-back-layer'
import { summarizeProjectDiff } from './project-diff-summary'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { Activity, FileDiff, Files, Link2, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
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
  // 摘要沿用菜单语言：细线分组、30px 行、计数用等宽数字；弱化的分组标签不抢主内容。
  const row = 'w-full justify-start gap-2 px-2 font-normal text-content'
  const count = 'ml-auto shrink-0 text-xs text-content-muted tabular-nums'
  const body = (
    <>
      {limit && (
        <p role="alert" className="px-2 pb-1 text-xs text-danger">
          {t('chat:sessionContext.pageLimit')}
        </p>
      )}
      <div className="flex min-w-0 items-start gap-2 border-b border-stroke-soft px-2 pt-1 pb-2">
        <div className="min-w-0 flex-1">
          <p className="truncate font-mono text-xs text-content-muted" title={cwd}>
            {cwd.split(/[\\/]/).filter(Boolean).at(-1) || cwd}
          </p>
          <p className="mt-0.5 truncate text-sm font-medium text-content">{taskName}</p>
        </div>
        <Button
          variant="ghost"
          size="icon"
          className="max-[650px]:size-11"
          aria-label={t('chat:workbench.closeSummary')}
          onClick={() => {
            close()
            restore()
          }}
        >
          <X size={16} />
        </Button>
      </div>
      <section className="border-b border-stroke-soft py-1">
        <Button
          variant="ghost"
          className={`${row} h-auto min-h-8 whitespace-normal text-left`}
          disabled={!vcsAvailable}
          onClick={() => open('changes')}
        >
          <FileDiff size={16} className="text-content-muted" />
          <span className="min-w-0 flex-1">{t('chat:workbench.projectChanges')}</span>
          <span className={count}>
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
          <p role="alert" className="px-2 py-1 text-xs text-danger">
            {t('chat:workbench.changesFailed')}
          </p>
        )}
        {changes.data?.isRepo && !changes.isError && !changes.data.error && (
          <p className="px-2 pb-1 pl-8 font-mono text-xs tabular-nums text-content-muted">
            {totals.partial ? `${t('chat:workbench.partialCounts')} ` : ''}
            <span className="text-success">+{totals.added}</span>{' '}
            <span className="text-danger">−{totals.removed}</span>
          </p>
        )}
        <Button variant="ghost" className={row} onClick={() => open('files')}>
          <Files size={16} className="text-content-muted" />
          {t('chat:workbench.taskSnapshots')}
        </Button>
      </section>
      <section className="border-b border-stroke-soft py-1">
        <Button variant="ghost" className={row} onClick={() => open('processes')}>
          <span>{t('terminal:workspace.processes')}</span>
          <span className={count}>{records.length}</span>
        </Button>
        {!service && (
          <p className="px-2 pb-1 text-xs text-content-muted">
            {t('terminal:workspace.nativeOnly')}
          </p>
        )}
        {records.slice(0, 3).map((record) => (
          <Button
            key={record.id}
            variant="ghost"
            className={`${row} min-w-0`}
            onClick={() => open('processes', record.id)}
          >
            {record.status === 'running' ? (
              <OrbitSpinner className="size-3.5" />
            ) : (
              <Activity size={15} className="text-content-muted" />
            )}
            <span className="min-w-0 flex-1 truncate text-left font-mono text-xs">
              {record.profileId}
            </span>
            <span className={count}>
              {record.status === 'running'
                ? t('terminal:workspace.running')
                : record.status === 'exited'
                  ? t('terminal:workspace.exited')
                  : t('chat:workbench.otherState')}
            </span>
          </Button>
        ))}
      </section>
      <section className="pt-1">
        <Button variant="ghost" className={row} onClick={() => open('sources')}>
          <span>{t('chat:workbench.sources')}</span>
          <span className={count}>{sources.length}</span>
        </Button>
        {sources.slice(0, 3).map((source) => (
          <Button
            key={source.id}
            variant="ghost"
            className={`${row} min-w-0`}
            onClick={() => open('sources', source.id)}
          >
            <Link2 size={15} className="text-content-muted" />
            <span className="truncate font-mono text-xs">
              {source.name || t('chat:chatMessage.fileAttachment')}
            </span>
          </Button>
        ))}
        <Button
          variant="ghost"
          className={`${row} text-content-muted`}
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
          className="max-h-[80dvh] overflow-y-auto rounded-t-2xl p-3 pb-[max(16px,var(--pisper-safe-area-bottom))] [&_button]:min-h-11"
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
      className="absolute top-[52px] right-2 z-20 max-h-[calc(100%-64px)] w-80 max-w-[calc(100%-16px)] overflow-y-auto rounded-md bg-popover p-1 text-sm text-popover-foreground shadow-float"
    >
      {body}
    </div>
  )
}
