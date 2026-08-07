import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Check,
  FileDiff,
  GitBranch,
  GitCommitHorizontal,
  RefreshCw,
  Undo2,
  Upload,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { workspaceName } from '@/lib/format'
import { chatApi, type GitChangesResponse } from './chat-api'
import { GitDiffDialog } from './GitDiffViewer'

const DEFAULT_COMMIT_MESSAGE = 'Agent changes'

type GitAction = 'commit' | 'push' | 'revert'

function isError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

export function GitChangesControl({
  sessionId,
  streaming,
}: {
  sessionId?: string
  streaming?: boolean
}) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  const [diffOpen, setDiffOpen] = useState(false)
  const [loading, setLoading] = useState(false)
  const [running, setRunning] = useState<GitAction | null>(null)
  const [confirmingRevert, setConfirmingRevert] = useState(false)
  const [commitMessage, setCommitMessage] = useState('')
  const [changes, setChanges] = useState<GitChangesResponse | null>(null)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    if (!sessionId) return
    setLoading(true)
    setError('')
    try {
      const data = await chatApi.getVcsChanges(sessionId)
      setChanges(data)
    } catch (caught) {
      setError(isError(caught))
    } finally {
      setLoading(false)
    }
  }, [sessionId])

  useEffect(() => {
    if (!open) return undefined
    void load()
    const close = (event: MouseEvent) => {
      const target = event.target instanceof Node ? event.target : null
      if (!rootRef.current?.contains(target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', close)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('mousedown', close)
      document.removeEventListener('keydown', escape)
    }
  }, [open, load])

  useEffect(() => {
    if (!streaming) void load().catch(() => {})
    // Refresh the change summary whenever a run finishes so the badge stays current.
  }, [streaming, load])

  const runAction = async (action: GitAction) => {
    if (!sessionId || running) return
    if (action === 'revert' && !confirmingRevert) {
      setConfirmingRevert(true)
      return
    }
    setRunning(action)
    setError('')
    setNotice('')
    try {
      let data: GitChangesResponse
      if (action === 'commit') {
        data = await chatApi.commitVcsChanges(
          sessionId,
          commitMessage.trim() || DEFAULT_COMMIT_MESSAGE,
        )
        setCommitMessage('')
        setNotice(t('chat:focusSession.gitCommitted'))
      } else if (action === 'push') {
        data = await chatApi.pushVcsChanges(sessionId)
        setNotice(t('chat:focusSession.gitPushed'))
      } else {
        data = await chatApi.revertVcsChanges(sessionId)
        setConfirmingRevert(false)
        setNotice(t('chat:focusSession.gitReverted'))
      }
      setChanges(data)
    } catch (caught) {
      setError(isError(caught))
    } finally {
      setRunning(null)
    }
  }

  const vcs = changes?.vcs || (changes?.isRepo ? 'git' : '')
  const isSvn = vcs === 'svn'
  const fileCount = changes?.isRepo ? changes.files.length : 0
  const hasChanges = fileCount > 0
  const hasCommitsToPush = Boolean(!isSvn && changes?.isRepo && (changes.ahead ?? 0) > 0)
  const busy = Boolean(running)
  const repoLabel = isSvn
    ? t('chat:focusSession.vcsSvnWorkspace')
    : changes?.branch || t('chat:focusSession.gitDetachedHead')
  const label = changes?.isRepo
    ? `${t('chat:focusSession.gitChanges')}${changes.branch ? ` · ${changes.branch}` : ''}${isSvn ? ` · ${t('chat:focusSession.vcsSvnWorkspace')}` : ''}${hasChanges ? ` · ${t('chat:focusSession.gitFilesChanged', { count: fileCount })}` : ''}`
    : t('chat:focusSession.gitChanges')

  return (
    <div
      ref={rootRef}
      className={`git-changes-select ${open ? 'open' : ''} ${hasChanges ? 'active' : ''}`}
    >
      <button
        type="button"
        className="git-changes-trigger"
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={!sessionId}
        onClick={() => setOpen((visible) => !visible)}
      >
        <GitBranch size={14} />
        {hasChanges && <i>{fileCount > 99 ? '99+' : fileCount}</i>}
      </button>
      {open && (
        <div
          className="git-changes-menu"
          role="dialog"
          aria-label={t('chat:focusSession.gitChanges')}
        >
          <div className="git-changes-menu-head">
            <span className="git-changes-menu-icon">
              <GitBranch size={15} />
            </span>
            <span>
              <strong>{t('chat:focusSession.gitChanges')}</strong>
              <small title={changes?.cwd || ''}>
                {changes?.isRepo
                  ? repoLabel
                  : loading
                    ? t('chat:focusSession.gitLoading')
                    : error && !changes
                      ? t('chat:focusSession.gitLoadFailed')
                      : changes?.gitAvailable === false && changes?.svnAvailable === false
                        ? t('chat:focusSession.gitUnavailable')
                        : `${t('chat:focusSession.gitNotARepository')}${changes?.cwd ? ` · ${workspaceName(changes.cwd, language)}` : ''}`}
              </small>
            </span>
            <button
              type="button"
              className="icon-button"
              title={t('chat:focusSession.gitRefresh')}
              aria-label={t('chat:focusSession.gitRefresh')}
              disabled={loading || busy}
              onClick={() => void load()}
            >
              <RefreshCw className={loading ? 'spin' : ''} size={13} />
            </button>
          </div>

          {error && <p className="git-changes-error">{error}</p>}
          {!error && changes?.error && <p className="git-changes-error">{changes.error}</p>}
          {notice && !error && <p className="git-changes-notice">{notice}</p>}

          {changes?.isRepo && (
            <>
              {hasChanges ? (
                <div className="git-changes-file-list">
                  {changes.files.slice(0, 60).map((file) => (
                    <div className="git-changes-file" key={`${file.status}-${file.path}`}>
                      <code className="git-changes-file-status">{file.status}</code>
                      <span title={file.path}>{file.path}</span>
                    </div>
                  ))}
                  {changes.files.length > 60 && (
                    <small>
                      {t('chat:focusSession.gitMoreFiles', { count: changes.files.length - 60 })}
                    </small>
                  )}
                </div>
              ) : (
                <p className="git-changes-empty">{t('chat:focusSession.gitNoChanges')}</p>
              )}

              {hasChanges && (
                <div className="git-changes-commit-row">
                  <input
                    value={commitMessage}
                    placeholder={t('chat:focusSession.gitCommitMessagePlaceholder')}
                    onChange={(event) => setCommitMessage(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter' && !event.shiftKey) {
                        event.preventDefault()
                        void runAction('commit')
                      }
                    }}
                  />
                  <button
                    type="button"
                    className="button primary tiny"
                    disabled={busy || streaming}
                    title={
                      streaming
                        ? t('chat:focusSession.gitWaitForRunToFinish')
                        : t('chat:focusSession.gitCommit')
                    }
                    onClick={() => void runAction('commit')}
                  >
                    {running === 'commit' ? (
                      <RefreshCw className="spin" size={12} />
                    ) : (
                      <GitCommitHorizontal size={12} />
                    )}
                    {t('chat:focusSession.gitCommit')}
                  </button>
                </div>
              )}

              {hasChanges && (
                <button
                  type="button"
                  className="git-changes-view-diff"
                  onClick={() => {
                    setDiffOpen(true)
                    setOpen(false)
                  }}
                >
                  <FileDiff size={13} />
                  {t('chat:focusSession.gitViewDiff')}
                </button>
              )}

              <div className="git-changes-actions">
                {!isSvn && (
                  <button
                    type="button"
                    className="button secondary tiny"
                    disabled={busy || (!hasCommitsToPush && !hasChanges)}
                    title={
                      hasCommitsToPush
                        ? t('chat:focusSession.gitPushAhead', { count: changes?.ahead || 0 })
                        : t('chat:focusSession.gitPush')
                    }
                    onClick={() => void runAction('push')}
                  >
                    {running === 'push' ? (
                      <RefreshCw className="spin" size={12} />
                    ) : (
                      <Upload size={12} />
                    )}
                    {t('chat:focusSession.gitPush')}
                    {hasCommitsToPush ? ` ↑${changes?.ahead}` : ''}
                  </button>
                )}
                {confirmingRevert ? (
                  <>
                    <button
                      type="button"
                      className="button danger tiny"
                      disabled={busy || streaming}
                      title={
                        streaming
                          ? t('chat:focusSession.gitWaitForRunToFinish')
                          : t('chat:focusSession.gitConfirmRevert')
                      }
                      onClick={() => void runAction('revert')}
                    >
                      {running === 'revert' ? (
                        <RefreshCw className="spin" size={12} />
                      ) : (
                        <Check size={12} />
                      )}
                      {t('chat:focusSession.gitConfirmRevert')}
                    </button>
                    <button
                      type="button"
                      className="button secondary tiny"
                      disabled={busy}
                      onClick={() => setConfirmingRevert(false)}
                    >
                      <X size={12} />
                      {t('chat:focusSession.gitCancel')}
                    </button>
                  </>
                ) : (
                  <button
                    type="button"
                    className="button secondary tiny"
                    disabled={busy || streaming || !hasChanges}
                    title={
                      isSvn
                        ? t('chat:focusSession.vcsRevertDescription')
                        : t('chat:focusSession.gitRevertDescription')
                    }
                    onClick={() => void runAction('revert')}
                  >
                    <Undo2 size={12} />
                    {t('chat:focusSession.gitRevert')}
                  </button>
                )}
              </div>
            </>
          )}
        </div>
      )}
      {diffOpen && changes && (
        <GitDiffDialog
          diff={changes.diff || ''}
          truncated={changes.diffTruncated}
          onClose={() => setDiffOpen(false)}
        />
      )}
    </div>
  )
}
