import { summarizeProjectDiff } from './project-diff-summary'
import { useResourceReading, type ResourceReadingPosition } from './use-resource-reading'
import { lazy, Suspense, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronRight, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { chatApi } from './chat-api'
import type { ContextPageView } from './session-context-pages'
const InlineDiff = lazy(() =>
  import('./GitDiffViewer').then((module) => ({ default: module.InlineGitDiff })),
)

export function TaskFileDiff({
  sessionId,
  path,
  scope,
  revision = 0,
  position,
  onReading,
}: {
  sessionId: string
  path: string
  scope: 'project' | 'task'
  revision?: number
  position?: ResourceReadingPosition
  onReading?: (position: ResourceReadingPosition) => void
}) {
  const { t } = useI18n()
  const query = useQuery<{ diff: string; diffTruncated?: boolean }>({
    queryKey: ['task-file-diff', sessionId, scope, path, revision],
    queryFn: ({ signal }) =>
      scope === 'project'
        ? chatApi.getFileDiff(sessionId, path, { signal })
        : chatApi.getSessionFileChangeDiff(sessionId, path, { signal }),
  })
  if (query.isPending)
    return (
      <p role="status" className="p-3 text-xs text-content-muted">
        {t('chat:focusSession.gitLoading')}
      </p>
    )
  if (query.isError)
    return (
      <div className="p-3">
        <p role="alert" className="text-xs text-destructive">
          {query.error.message}
        </p>
        <Button variant="ghost" size="sm" onClick={() => void query.refetch()}>
          {t('chat:chatMessage.filePreviewRetry')}
        </Button>
      </div>
    )
  return (
    <Suspense fallback={null}>
      <InlineDiff
        position={position}
        onReading={onReading}
        diff={query.data.diff}
        truncated={query.data.diffTruncated}
      />
    </Suspense>
  )
}

export function TaskChangesPane({
  sessionId,
  cwd,
  streaming,
  view,
  onViewChange,
}: {
  sessionId: string
  cwd: string
  streaming: boolean
  view: ContextPageView
  onViewChange: (patch: Partial<ContextPageView>) => void
}) {
  const { t } = useI18n()
  const query = useQuery({
    queryKey: ['task-project-changes', sessionId, cwd, streaming],
    queryFn: ({ signal }) => chatApi.getVcsChanges(sessionId, { signal }),
  })
  const reading = useResourceReading<HTMLDivElement>(view.reading?.list, (position) =>
    onViewChange({ reading: { ...view.reading, list: position } }),
  )
  const totals = useMemo(
    () =>
      summarizeProjectDiff(
        query.data?.diff || '',
        query.data?.files.map((file) => file.path) || [],
        query.data?.diffTruncated,
      ),
    [query.data],
  )
  const missing = view.expandedFiles.filter(
    (path) => query.data && !query.data.files.some((file) => file.path === path),
  )
  const error = query.error?.message || query.data?.error
  const files = query.data?.files || []
  return (
    <section
      aria-label={t('chat:workbench.projectChanges')}
      className="flex min-h-0 flex-1 flex-col gap-3 p-3"
    >
      <header className="flex min-w-0 items-center gap-2">
        <h2 className="flex-1 text-sm font-medium">{t('chat:workbench.projectChanges')}</h2>
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t('chat:focusSession.gitRefresh')}
          disabled={query.isFetching}
          onClick={() => void query.refetch()}
        >
          <RefreshCw size={15} />
        </Button>
      </header>
      <p className="text-xs text-content-muted">{t('chat:workbench.projectScope')}</p>
      <p className="truncate text-xs text-content-muted" title={cwd}>
        {query.data?.branch ? `${query.data.branch} · ` : ''}
        {cwd}
      </p>
      <div className="flex gap-2">
        <Button
          variant="outline"
          size="sm"
          disabled={!files.length || !!error}
          onClick={() => onViewChange({ expandedFiles: files.map((file) => file.path) })}
        >
          {t('chat:workbench.expandAll')}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => onViewChange({ expandedFiles: [] })}>
          {t('chat:workbench.collapseAll')}
        </Button>
      </div>
      {!query.isFetching &&
        !error &&
        missing.map((path) => (
          <p key={path} role="status" className="break-all text-xs text-content-muted">
            {t('chat:workbench.fileNoLongerChanged', { path })}
          </p>
        ))}
      {query.isPending ? (
        <p role="status" className="text-sm text-content-muted">
          {t('chat:focusSession.gitLoading')}
        </p>
      ) : error ? (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : !query.data?.isRepo ? (
        <p className="text-sm text-content-muted">{t('chat:workbench.notRepository')}</p>
      ) : !files.length ? (
        <p className="text-sm text-content-muted">{t('chat:focusSession.fileChangesEmpty')}</p>
      ) : (
        <div ref={reading.ref} className="min-h-0 flex-1 overflow-y-auto">
          {files.map((file) => {
            const expanded = view.expandedFiles.includes(file.path)
            return (
              <article key={file.path} className="border-b border-border/60">
                <button
                  aria-expanded={expanded}
                  title={file.path}
                  className="flex min-h-11 w-full min-w-0 items-center gap-2 text-left text-sm"
                  onClick={() =>
                    onViewChange({
                      expandedFiles: expanded
                        ? view.expandedFiles.filter((path) => path !== file.path)
                        : [...view.expandedFiles, file.path],
                    })
                  }
                >
                  <ChevronRight
                    size={15}
                    className={expanded ? 'shrink-0 rotate-90' : 'shrink-0'}
                  />
                  <span className="min-w-0 flex-1 truncate">{file.path}</span>
                  <span className="shrink-0 text-xs text-content-muted">{file.status}</span>
                  {Object.hasOwn(totals.files, file.path) && (
                    <span className="shrink-0 text-xs text-content-muted">
                      +{totals.files[file.path].added} −{totals.files[file.path].removed}
                    </span>
                  )}
                </button>
                {expanded && (
                  <p className="break-all p-2 text-xs text-content-muted">{file.path}</p>
                )}
                {expanded && (
                  <TaskFileDiff
                    sessionId={sessionId}
                    path={file.path}
                    scope="project"
                    position={view.reading?.[file.path]}
                    onReading={(position) =>
                      onViewChange({ reading: { ...view.reading, [file.path]: position } })
                    }
                    revision={query.dataUpdatedAt}
                  />
                )}
              </article>
            )
          })}
        </div>
      )}
    </section>
  )
}
