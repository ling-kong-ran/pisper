// 文件快照的读取、审批和撤销由当前面板持有；切换会话时整组状态重新挂载。
import { lazy, Suspense, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Check, FileDiff, RefreshCw, Undo2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import { chatApi, type SessionFileChangesResponse } from './chat-api'
import { invalidateSessionChangeSummary } from './session-change-summary-api'

const GitDiffDialog = lazy(() =>
  import('./GitDiffViewer').then((module) => ({ default: module.GitDiffDialog })),
)
const MAX_VISIBLE_FILES = 100

type FileAction = { kind: 'approve' | 'revert'; path?: string }
type SessionFilesPaneProps = {
  sessionId: string
  streaming: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

export function SessionFilesPane({ sessionId, streaming, requestConfirm }: SessionFilesPaneProps) {
  const { t } = useI18n()
  const [revision, setRevision] = useState(0)
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
          if (activeRef.current && sequence === requestSequence.current) setChanges(data)
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

  return (
    <section
      className="flex min-h-0 flex-1 flex-col gap-3 p-3"
      aria-label={t('chat:focusSession.fileChanges')}
      aria-busy={loading || Boolean(running)}
    >
      <div className="flex flex-none flex-wrap items-center justify-between gap-2">
        <div className="min-w-0">
          <p className="text-[length:var(--app-font-size)] font-medium">
            {t('chat:focusSession.fileChanges')}
          </p>
          {changes && (
            <p className="text-[length:var(--app-small-size)] text-[var(--text-muted)]">
              {t('chat:focusSession.fileChangesCount', { count: changes.summary.files })}
              {' · '}+{changes.summary.added} −{changes.summary.removed}
              {pendingCount > 0 &&
                ` · ${t('chat:focusSession.fileChangesPending', { count: pendingCount })}`}
            </p>
          )}
        </div>
        <div className="ml-auto flex flex-wrap items-center justify-end gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="min-h-11 sm:min-h-9"
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
            variant="outline"
            size="sm"
            className="min-h-11 sm:min-h-9"
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
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 sm:size-9"
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
            <RefreshCw className={loading ? 'animate-spin' : ''} size={16} />
          </Button>
        </div>
      </div>
      {displayedError && (
        <p
          role="alert"
          className="text-[length:var(--app-small-size)] text-[var(--text-secondary)]"
        >
          {displayedError}
        </p>
      )}
      {notice && !displayedError && (
        <p
          role="status"
          className="text-[length:var(--app-small-size)] text-[var(--text-secondary)]"
        >
          {notice}
        </p>
      )}
      {loading && !changes ? (
        <p role="status" className="text-[length:var(--app-font-size)] text-[var(--text-muted)]">
          {t('chat:focusSession.gitLoading')}
        </p>
      ) : !changes?.files.length ? (
        !displayedError && (
          <p className="text-[length:var(--app-font-size)] text-[var(--text-muted)]">
            {t('chat:focusSession.fileChangesEmpty')}
          </p>
        )
      ) : (
        <ul
          className="min-h-0 flex-1 space-y-1 overflow-y-auto"
          aria-label={t('chat:focusSession.fileChanges')}
        >
          {changes.files.slice(0, MAX_VISIBLE_FILES).map((file) => (
            <li
              key={file.path}
              className="flex min-w-0 items-center gap-1 rounded-md bg-[var(--surface-subtle)] px-2 py-1"
            >
              <button
                type="button"
                className="flex min-h-11 min-w-0 flex-1 items-center gap-2 rounded-md text-left text-[length:var(--app-small-size)] outline-none focus-visible:ring-2 focus-visible:ring-[var(--brand-blue)] disabled:opacity-60"
                title={file.path}
                disabled={loading || Boolean(running)}
                aria-busy={diffLoading === file.path}
                onClick={() => void viewDiff(file.path)}
              >
                {diffLoading === file.path ? (
                  <RefreshCw size={14} className="flex-none animate-spin" />
                ) : (
                  <FileDiff size={14} className="flex-none text-[var(--star-strong)]" />
                )}
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{file.path}</span>
                  <span className="block text-[var(--text-muted)]">
                    +{file.added} −{file.removed}
                    {file.reverted
                      ? ` · ${t('chat:focusSession.fileChangesReverted')}`
                      : file.approved
                        ? ` · ${t('chat:focusSession.fileChangesApproved')}`
                        : ''}
                  </span>
                </span>
              </button>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="size-11 sm:size-9"
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
                className="size-11 sm:size-9"
                title={t('chat:focusSession.fileChangesRevertFile')}
                aria-label={`${t('chat:focusSession.fileChangesRevertFile')} ${file.path}`}
                disabled={writesDisabled || file.reverted || !file.canRevert}
                onClick={() => void runAction({ kind: 'revert', path: file.path })}
              >
                <Undo2 size={14} />
              </Button>
            </li>
          ))}
          {changes.files.length > MAX_VISIBLE_FILES && (
            <li className="px-2 py-1 text-[length:var(--app-small-size)] text-[var(--text-muted)]">
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
