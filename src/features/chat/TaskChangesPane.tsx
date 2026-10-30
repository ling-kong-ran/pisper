import { summarizeProjectDiff } from './project-diff-summary'
import { useResourceReading, type ResourceReadingPosition } from './use-resource-reading'
import { lazy, Suspense, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { ChevronRight, GitBranch, RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { OrbitSpinner } from '@/components/ui/spinner'
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
  mode,
  fill = false,
}: {
  sessionId: string
  path: string
  scope: 'project' | 'task'
  revision?: number
  position?: ResourceReadingPosition
  onReading?: (position: ResourceReadingPosition) => void
  // 检查器“改动”详情区：统一 / 并排视图，并让差异区填满剩余高度。
  mode?: 'unified' | 'split'
  fill?: boolean
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
      <p role="status" className="flex items-center gap-2 px-3 py-2.5 text-xs text-content-muted">
        <OrbitSpinner className="size-3.5" />
        {t('chat:focusSession.gitLoading')}
      </p>
    )
  if (query.isError)
    return (
      <div className="px-3 py-2.5">
        <p role="alert" className="text-xs text-danger">
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
        mode={mode}
        fill={fill}
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
      className="flex min-h-0 flex-1 flex-col"
    >
      {/* 细线列表：12/500 弱化分区标签 + 等宽数字元信息；路径一律等宽字体，增删统计用成功/危险色。 */}
      <header className="flex min-h-10 min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft pr-1.5 pl-3">
        <h2 className="min-w-0 flex-1 truncate text-xs font-medium tracking-[.02em] text-content-muted">
          {t('chat:workbench.projectChanges')}
        </h2>
        {query.data?.isRepo && !error && (
          <span className="shrink-0 font-mono text-xs tabular-nums">
            <span className="text-success">+{totals.added}</span>{' '}
            <span className="text-danger">−{totals.removed}</span>
          </span>
        )}
        <Button
          variant="ghost"
          size="icon"
          className="max-[650px]:size-11"
          aria-label={t('chat:focusSession.gitRefresh')}
          disabled={query.isFetching}
          onClick={() => void query.refetch()}
        >
          <RefreshCw size={15} className={query.isFetching ? 'animate-spin' : undefined} />
        </Button>
      </header>
      <div className="flex shrink-0 flex-col gap-2 border-b border-stroke-soft px-3 py-2.5">
        <p className="text-xs leading-relaxed text-content-muted">
          {t('chat:workbench.projectScope')}
        </p>
        <p className="flex min-w-0 items-center gap-1.5 text-xs" title={cwd}>
          {query.data?.branch && (
            <span className="inline-flex h-5.5 max-w-[45%] shrink-0 items-center gap-1 rounded-xs border border-stroke px-1.5 font-mono text-content-secondary">
              <GitBranch size={12} className="shrink-0" aria-hidden="true" />
              <span className="truncate">{query.data.branch}</span>
            </span>
          )}
          <span className="min-w-0 truncate font-mono text-content-muted">{cwd}</span>
        </p>
        <div className="flex gap-1">
          <Button
            variant="ghost"
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
      </div>
      {!query.isFetching &&
        !error &&
        missing.map((path) => (
          <p key={path} role="status" className="break-all px-3 pt-2 text-xs text-content-muted">
            {t('chat:workbench.fileNoLongerChanged', { path })}
          </p>
        ))}
      {query.isPending ? (
        <PageState kind="loading" title={t('chat:focusSession.gitLoading')} />
      ) : error ? (
        <p role="alert" className="p-3 text-sm text-danger">
          {error}
        </p>
      ) : !query.data?.isRepo ? (
        <InlineEmptyState compact title={t('chat:workbench.notRepository')} />
      ) : !files.length ? (
        <InlineEmptyState compact title={t('chat:focusSession.fileChangesEmpty')} />
      ) : (
        <div ref={reading.ref} className="min-h-0 flex-1 overflow-y-auto">
          {files.map((file) => {
            const expanded = view.expandedFiles.includes(file.path)
            return (
              <article key={file.path} className="border-b border-stroke-soft">
                <button
                  aria-expanded={expanded}
                  title={file.path}
                  className={`flex min-h-9 w-full min-w-0 items-center gap-2 pr-3 pl-2 text-left text-sm hover:bg-surface-subtle focus-visible:-outline-offset-2 max-[650px]:min-h-11 pointer-coarse:min-h-11 ${expanded ? 'bg-surface-subtle' : ''}`}
                  onClick={() =>
                    onViewChange({
                      expandedFiles: expanded
                        ? view.expandedFiles.filter((path) => path !== file.path)
                        : [...view.expandedFiles, file.path],
                    })
                  }
                >
                  <ChevronRight
                    size={14}
                    className={`shrink-0 text-content-muted transition-transform duration-(--d-fast) ${expanded ? 'rotate-90' : ''}`}
                  />
                  {/* 可访问名称保持「路径 状态」顺序，供工作台冒烟按名称定位。 */}
                  <span className="min-w-0 flex-1 truncate font-mono text-xs">{file.path}</span>
                  <span className="min-w-4 shrink-0 text-center font-mono text-xs font-medium text-content-secondary">
                    {file.status}
                  </span>
                  {Object.hasOwn(totals.files, file.path) && (
                    <span className="shrink-0 font-mono text-xs tabular-nums">
                      <span className="text-success">+{totals.files[file.path].added}</span>{' '}
                      <span className="text-danger">−{totals.files[file.path].removed}</span>
                    </span>
                  )}
                </button>
                {expanded && (
                  <p className="break-all px-3 py-1.5 font-mono text-xs text-content-muted">
                    {file.path}
                  </p>
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
