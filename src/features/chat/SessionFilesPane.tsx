// @public 壳层可用的文件改动面板；调用方提供会话 ID、实时运行状态和确认能力。
import { useResourceReading } from './use-resource-reading'
// 文件快照的读取、审批和撤销由当前面板持有；切换会话时整组状态重新挂载。
import { lazy, Suspense, useState } from 'react'
import { Check, ChevronRight, FileDiff, RefreshCw, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { TaskFileDiff } from './TaskChangesPane'
import type { ContextPageView } from './session-context-pages'
import { useSessionFileChanges } from './use-session-file-changes'

const GitDiffDialog = lazy(() =>
  import('./GitDiffViewer').then((module) => ({ default: module.GitDiffDialog })),
)
const MAX_VISIBLE_FILES = 100

type SessionFilesPaneProps = {
  sessionId: string
  view?: ContextPageView
  onViewChange?: (patch: Partial<ContextPageView>) => void
  streaming: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

export function SessionFilesPane({
  sessionId,
  streaming,
  requestConfirm,
  view,
  onViewChange,
}: SessionFilesPaneProps) {
  const { t } = useI18n()
  const reading = useResourceReading<HTMLUListElement>(view?.reading?.list, (position) =>
    onViewChange?.({ reading: { ...view?.reading, list: position } }),
  )
  const [localExpanded, setLocalExpanded] = useState<string[]>([])
  const expandedFiles = view?.expandedFiles || localExpanded
  const expand = (paths: string[]) =>
    onViewChange ? onViewChange({ expandedFiles: paths }) : setLocalExpanded(paths)
  const {
    changes,
    loading,
    running,
    diff,
    setDiff,
    diffLoading,
    snapshotRevision,
    writesDisabled,
    pendingCount,
    reversibleCount,
    displayedError,
    notice,
    viewDiff,
    runAction,
    refresh,
  } = useSessionFileChanges({ sessionId, streaming, requestConfirm })

  // 触屏与窄屏保留 44px 触控目标；桌面行内操作是 28px 幽灵图标键。
  const rowAction = 'max-[650px]:size-11 pointer-coarse:size-11'
  return (
    <section
      className="flex min-h-0 flex-1 flex-col"
      aria-label={t('chat:focusSession.fileChanges')}
      aria-busy={loading || Boolean(running)}
    >
      {/* 细线分区：12/500 弱化标签 + 等宽数字统计；批量操作在下方工具行，路径一律等宽字体。 */}
      <header className="flex min-h-10 min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft pr-1.5 pl-3">
        <h2 className="min-w-0 flex-1 truncate text-xs font-medium tracking-[.02em] text-content-muted">
          {t('chat:focusSession.fileChanges')}
        </h2>
        {changes && (
          <span className="shrink-0 font-mono text-xs tabular-nums">
            <span className="text-success">+{changes.summary.added}</span>{' '}
            <span className="text-danger">−{changes.summary.removed}</span>
          </span>
        )}
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className={rowAction}
          aria-label={t('chat:focusSession.gitRefresh')}
          title={t('chat:focusSession.gitRefresh')}
          disabled={loading || Boolean(running)}
          onClick={refresh}
        >
          <RefreshCw className={loading ? 'animate-spin' : ''} size={15} />
        </Button>
      </header>
      <div className="flex shrink-0 flex-col gap-2 border-b border-stroke-soft px-3 py-2.5">
        {changes && (
          <p className="text-xs text-content-secondary tabular-nums">
            {t('chat:focusSession.fileChangesCount', { count: changes.summary.files })}
            {pendingCount > 0 &&
              ` · ${t('chat:focusSession.fileChangesPending', { count: pendingCount })}`}
          </p>
        )}
        <p className="text-xs leading-relaxed text-content-muted">
          {t('chat:workbench.taskSnapshotScope')}
        </p>
        <div className="flex flex-wrap items-center justify-between gap-1">
          <div className="flex gap-1">
            <Button
              type="button"
              variant="secondary"
              size="sm"
              disabled={writesDisabled || pendingCount === 0}
              title={streaming ? t('chat:focusSession.gitWaitForRunToFinish') : undefined}
              onClick={() => void runAction({ kind: 'approve' })}
            >
              {running?.kind === 'approve' && !running.path ? (
                <RefreshCw className="animate-spin" size={14} />
              ) : (
                <Check size={14} />
              )}
              {t('chat:focusSession.fileChangesApproveAll')}
            </Button>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              disabled={writesDisabled || reversibleCount === 0}
              title={
                streaming
                  ? t('chat:focusSession.gitWaitForRunToFinish')
                  : t('chat:focusSession.fileChangesRevertAllDescription')
              }
              onClick={() => void runAction({ kind: 'revert' })}
            >
              {running?.kind === 'revert' && !running.path ? (
                <RefreshCw className="animate-spin" size={14} />
              ) : (
                <Undo2 size={14} />
              )}
              {t('chat:focusSession.fileChangesRevertAll')}
            </Button>
          </div>
          <div className="flex gap-1">
            <Button
              variant="ghost"
              size="sm"
              disabled={loading || !changes?.files.length}
              onClick={() =>
                expand(changes?.files.slice(0, MAX_VISIBLE_FILES).map((file) => file.path) || [])
              }
            >
              {t('chat:workbench.expandAll')}
            </Button>
            <Button variant="ghost" size="sm" onClick={() => expand([])}>
              {t('chat:workbench.collapseAll')}
            </Button>
          </div>
        </div>
      </div>
      {displayedError && (
        <p role="alert" className="px-3 pt-2.5 text-xs text-danger">
          {displayedError}
        </p>
      )}
      {notice && !displayedError && (
        <p role="status" className="px-3 pt-2.5 text-xs text-content-secondary">
          {notice}
        </p>
      )}
      {loading && !changes ? (
        <PageState kind="loading" title={t('chat:focusSession.gitLoading')} />
      ) : !changes?.files.length ? (
        !displayedError && (
          <InlineEmptyState compact title={t('chat:focusSession.fileChangesEmpty')} />
        )
      ) : (
        <ul
          ref={reading.ref}
          className="min-h-0 flex-1 overflow-y-auto"
          aria-label={t('chat:focusSession.fileChanges')}
        >
          {changes.files.slice(0, MAX_VISIBLE_FILES).map((file) => {
            const expanded = expandedFiles.includes(file.path)
            return (
              <li key={file.path} className="min-w-0 border-b border-stroke-soft">
                <div
                  className={`flex min-w-0 items-center gap-0.5 pr-1.5 hover:bg-surface-subtle ${expanded ? 'bg-surface-subtle' : ''}`}
                >
                  <button
                    type="button"
                    className="flex min-h-9 min-w-0 flex-1 items-center gap-2 pl-2 text-left text-xs focus-visible:-outline-offset-2 disabled:opacity-60 max-[650px]:min-h-11 pointer-coarse:min-h-11"
                    title={file.path}
                    disabled={loading || Boolean(running)}
                    aria-busy={diffLoading === file.path}
                    aria-expanded={expanded}
                    onClick={() =>
                      expand(
                        expanded
                          ? expandedFiles.filter((path) => path !== file.path)
                          : [...expandedFiles, file.path],
                      )
                    }
                  >
                    <ChevronRight
                      size={14}
                      className={`shrink-0 text-content-muted transition-transform duration-(--d-fast) ${expanded ? 'rotate-90' : ''}`}
                    />
                    {diffLoading === file.path ? (
                      <OrbitSpinner className="size-3.5" />
                    ) : (
                      <FileDiff size={14} className="flex-none text-content-muted" />
                    )}
                    <span className="min-w-0 flex-1 truncate font-mono">{file.path}</span>
                    {(file.reverted || file.approved) && (
                      <span className="shrink-0 text-content-muted">
                        {file.reverted
                          ? t('chat:focusSession.fileChangesReverted')
                          : t('chat:focusSession.fileChangesApproved')}
                      </span>
                    )}
                    <span className="shrink-0 font-mono tabular-nums">
                      <span className="text-success">+{file.added}</span>{' '}
                      <span className="text-danger">−{file.removed}</span>
                    </span>
                  </button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className={rowAction}
                    title={t('chat:focusSession.fileChangesApproveFile')}
                    aria-label={`${t('chat:focusSession.fileChangesApproveFile')} ${file.path}`}
                    disabled={writesDisabled || !file.pending || file.approved || file.reverted}
                    onClick={() => void runAction({ kind: 'approve', path: file.path })}
                  >
                    <Check size={14} />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className={rowAction}
                    title={t('chat:focusSession.fileChangesRevertFile')}
                    aria-label={`${t('chat:focusSession.fileChangesRevertFile')} ${file.path}`}
                    disabled={writesDisabled || file.reverted || !file.canRevert}
                    onClick={() => void runAction({ kind: 'revert', path: file.path })}
                  >
                    <Undo2 size={14} />
                  </Button>
                </div>
                {expanded && (
                  <>
                    <TaskFileDiff
                      sessionId={sessionId}
                      path={file.path}
                      scope="task"
                      position={view?.reading?.[file.path]}
                      onReading={(position) =>
                        onViewChange?.({ reading: { ...view?.reading, [file.path]: position } })
                      }
                      revision={snapshotRevision}
                    />
                    <Button
                      variant="ghost"
                      size="sm"
                      className="m-1.5"
                      onClick={() => void viewDiff(file.path)}
                    >
                      {t('chat:workbench.fullDiff')}
                    </Button>
                  </>
                )}
              </li>
            )
          })}
          {changes.files.length > MAX_VISIBLE_FILES && (
            <li className="px-3 py-2 text-xs text-content-muted tabular-nums">
              {t('chat:focusSession.gitMoreFiles', {
                count: changes.files.length - MAX_VISIBLE_FILES,
              })}
            </li>
          )}
        </ul>
      )}
      {diff && (
        <Suspense fallback={null}>
          <GitDiffDialog
            diff={diff.diff}
            truncated={diff.truncated}
            onClose={() => setDiff(null)}
          />
        </Suspense>
      )}
    </section>
  )
}
