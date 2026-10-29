// @public 壳层可用的文件改动面板；调用方提供会话 ID、实时运行状态和确认能力。
import { useResourceReading } from './use-resource-reading'
// 文件快照的读取、审批和撤销由当前面板持有；切换会话时整组状态重新挂载。
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Check, ChevronRight, FileDiff, RefreshCw, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { chatApi, type SessionFileChangesResponse } from './chat-api'
import { TaskFileDiff } from './TaskChangesPane'
import type { ContextPageView } from './session-context-pages'
import { invalidateSessionChangeSummary } from './session-change-summary-api'

const GitDiffDialog = lazy(() =>
  import('./GitDiffViewer').then((module) => ({ default: module.GitDiffDialog })),
)
const MAX_VISIBLE_FILES = 100

type FileAction = { kind: 'approve' | 'revert'; path?: string }
type SessionFilesPaneProps = {
  sessionId: string
  view?: ContextPageView
  onViewChange?: (patch: Partial<ContextPageView>) => void
  streaming: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
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
  const [revision, setRevision] = useState(0)
  const [snapshotRevision, setSnapshotRevision] = useState(0)
  const [changes, setChanges] = useState<SessionFileChangesResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [actionError, setActionError] = useState('')
  const [notice, setNotice] = useState('')
  const [running, setRunning] = useState<FileAction | null>(null)
  const [diff, setDiff] = useState<{ diff: string; truncated: boolean } | null>(null)
  const [diffLoading, setDiffLoading] = useState<string | null>(null)
  const activeRef = useRef(false)
  const requestSequence = useRef(0)
  const diffSequence = useRef(0)
  const loadingRef = useRef(true)
  const runningRef = useRef(false)
  const streamingRef = useRef(streaming)

  useLayoutEffect(() => {
    streamingRef.current = streaming
  }, [streaming])

  // 确认框可以跨面板存活，卸载提交阶段就失效，不能等后续异步清理。
  useLayoutEffect(() => {
    activeRef.current = true
    return () => {
      activeRef.current = false
      requestSequence.current += 1
      diffSequence.current += 1
    }
  }, [])

  useEffect(() => {
    const sequence = ++requestSequence.current
    diffSequence.current += 1
    setDiff(null)
    setDiffLoading(null)
    loadingRef.current = true
    setLoading(true)
    setError('')
    void chatApi
      .getSessionFileChanges(sessionId)
      .then(
        (data) => {
          if (activeRef.current && sequence === requestSequence.current) {
            setChanges(data)
            setSnapshotRevision((value) => value + 1)
          }
        },
        (caught: unknown) => {
          if (activeRef.current && sequence === requestSequence.current)
            setError(errorMessage(caught))
        },
      )
      .finally(() => {
        if (activeRef.current && sequence === requestSequence.current) {
          loadingRef.current = false
          setLoading(false)
        }
      })
    return () => {
      requestSequence.current += 1
    }
  }, [revision, sessionId, streaming])

  const viewDiff = async (path: string) => {
    if (runningRef.current || loadingRef.current) return
    const sequence = ++diffSequence.current
    setDiffLoading(path)
    setActionError('')
    try {
      const result = await chatApi.getSessionFileChangeDiff(sessionId, path)
      if (!activeRef.current || sequence !== diffSequence.current) return
      if (!result.diff?.trim()) {
        setActionError(t('chat:focusSession.fileChangesNoSnapshot'))
        return
      }
      setDiff({ diff: result.diff, truncated: Boolean(result.diffTruncated) })
    } catch (caught) {
      if (activeRef.current && sequence === diffSequence.current)
        setActionError(errorMessage(caught))
    } finally {
      if (activeRef.current && sequence === diffSequence.current) setDiffLoading(null)
    }
  }

  const runAction = async (action: FileAction) => {
    if (
      !activeRef.current ||
      runningRef.current ||
      loadingRef.current ||
      streamingRef.current ||
      !changes ||
      error
    )
      return
    const eligibleFiles = changes.files.filter((file) =>
      action.kind === 'approve'
        ? file.pending && !file.approved && !file.reverted
        : file.canRevert && !file.reverted,
    )
    if (!eligibleFiles.some((file) => action.path === undefined || file.path === action.path))
      return

    // 同步锁同时覆盖确认和写入，防止双击、单文件与批量操作彼此交错。
    runningRef.current = true
    setRunning(action)
    setActionError('')
    setNotice('')
    const sequence = ++requestSequence.current
    diffSequence.current += 1
    setDiff(null)
    setDiffLoading(null)
    let attempted = false
    try {
      if (action.kind === 'revert') {
        const confirmed = await requestConfirm({
          title: action.path
            ? t('chat:focusSession.fileChangesRevertFile')
            : t('chat:focusSession.fileChangesRevertAll'),
          message: action.path
            ? t('chat:focusSession.fileChangesRevertFileDescription', { path: action.path })
            : t('chat:focusSession.fileChangesRevertAllDescription'),
          confirmLabel: t('chat:focusSession.fileChangesConfirmRevert'),
          tone: 'danger',
        })
        if (!confirmed) return
      }
      // 确认期间会话可能开始新一轮运行，不能再提交针对旧快照的撤销。
      if (
        !activeRef.current ||
        streamingRef.current ||
        loadingRef.current ||
        sequence !== requestSequence.current
      )
        return
      attempted = true
      const result =
        action.kind === 'approve'
          ? await chatApi.approveSessionFileChanges(sessionId, action.path)
          : await chatApi.revertSessionFileChanges(sessionId, action.path)
      // 写入属于会话本身；即使面板已卸载，也要让其他摘要观察者重新读取。
      void invalidateSessionChangeSummary(sessionId)
      if (!activeRef.current || sequence !== requestSequence.current) return
      setChanges(result)
      if (action.kind === 'approve') {
        setNotice(
          action.path
            ? t('chat:focusSession.fileChangesApproved')
            : t('chat:focusSession.fileChangesApproveDone'),
        )
      } else if (!result.reverted) {
        setNotice(t('chat:focusSession.fileChangesRevertUnavailable'))
      } else if (action.path === undefined && result.files.some((file) => file.pending)) {
        setNotice(t('chat:focusSession.fileChangesRevertPartial', { count: result.reverted }))
      } else {
        setNotice(t('chat:focusSession.fileChangesRevertDone'))
      }
    } catch (caught) {
      if (activeRef.current && sequence === requestSequence.current)
        setActionError(errorMessage(caught))
    } finally {
      runningRef.current = false
      if (activeRef.current) {
        setRunning(null)
        // 运行状态变化或网络错误时都重新取快照，旧读取不能覆盖写入结果。
        if (attempted) {
          loadingRef.current = true
          setLoading(true)
          setRevision((current) => current + 1)
        }
      }
    }
  }

  const writesDisabled = loading || streaming || Boolean(running) || Boolean(error) || !changes
  const pendingCount = changes?.summary.pending || 0
  const reversibleCount =
    changes?.files.filter((file) => file.canRevert && !file.reverted).length || 0
  const displayedError = actionError || error

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
          onClick={() => {
            loadingRef.current = true
            setLoading(true)
            setActionError('')
            setRevision((current) => current + 1)
          }}
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
