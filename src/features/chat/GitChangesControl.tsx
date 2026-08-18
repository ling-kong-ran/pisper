// Git 变更控制：暂存/提交等 Git 操作面板，展示变更统计。
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
import { useViewportMenuOffset } from './use-viewport-menu-offset'

import { Button } from '@/components/ui/button'

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
  const menuRef = useRef<HTMLDivElement>(null)

  useViewportMenuOffset(open, menuRef)

  // 加载 Git 变更摘要（文件数/待推送/是否仓库），供面板与徽标使用。
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

  // 执行 Git/VCS 动作：commit/push/revert（revert 先进入二次确认态），
  // 成功后刷新变更摘要并提示。
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
      className={`git-changes-select [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] relative flex-none w-[38px] h-[38px] text-[var(--text-tertiary)] ${open ? 'open' : ''}    ${hasChanges ? 'active' : ''}`}
    >
      <button
        type="button"
        className="git-changes-trigger hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] [.git-changes-select.open_&]:border-[var(--accent-border)] [.git-changes-select.open_&]:bg-[var(--accent-soft)] [.git-changes-select.open_&]:text-[var(--star-strong)] [.git-changes-select.active_&]:text-[var(--star-strong)] [&_>_i]:absolute [&_>_i]:top-[-4px] [&_>_i]:right-[-6px] [&_>_i]:min-w-[15px] [&_>_i]:rounded-[var(--r-pill)] [&_>_i]:bg-[var(--star-strong)] [&_>_i]:p-[1px_4px] [&_>_i]:text-[var(--on-accent)] [&_>_i]:text-[9px] [&_>_i]:[font-style:normal] [&_>_i]:font-[700] [&_>_i]:leading-[1.3] [&_>_i]:text-center @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] relative grid w-full h-full place-items-center [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-inherit cursor-pointer"
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
          ref={menuRef}
          className="git-changes-menu [translate:var(--menu-x-offset,_0px)_0] max-[650px]:[.focus-composer_&]:right-[auto] max-[650px]:[.focus-composer_&]:left-0 max-[650px]:[.focus-composer_&]:w-[min(340px,calc(100vw_-_76px))] absolute z-[35] [bottom:calc(100%_+_8px)] left-0 w-[min(340px,calc(100vw_-_28px))] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)]"
          role="dialog"
          aria-label={t('chat:focusSession.gitChanges')}
        >
          <div className="git-changes-menu-head hover:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[12px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap grid min-h-[44px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] rounded-[var(--r-sm)] [padding:6px_7px]">
            <span className="git-changes-menu-icon [.git-changes-select.active_&]:bg-[var(--star-soft)] [.git-changes-select.active_&]:text-[var(--star-strong)] grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-[var(--text-muted)]">
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
            <Button
              type="button"
              variant="ghost"
              size="icon"
              title={t('chat:focusSession.gitRefresh')}
              aria-label={t('chat:focusSession.gitRefresh')}
              disabled={loading || busy}
              onClick={() => void load()}
            >
              <RefreshCw className={loading ? 'animate-spin' : ''} size={13} />
            </Button>
          </div>

          {error && (
            <p className="[margin:2px_7px_4px] text-[var(--danger,_#d64545)] text-[11px]">
              {error}
            </p>
          )}
          {!error && changes?.error && (
            <p className="[margin:2px_7px_4px] text-[var(--danger,_#d64545)] text-[11px]">
              {changes.error}
            </p>
          )}
          {notice && !error && (
            <p className="[margin:2px_7px_4px] text-[var(--success,_#2e9e63)] text-[11px]">
              {notice}
            </p>
          )}

          {changes?.isRepo && (
            <>
              {hasChanges ? (
                <div className="git-changes-file-list [&_>_small]:p-[3px_5px] [&_>_small]:text-[var(--text-muted)] flex max-h-[168px] flex-col gap-[1px] [margin:2px_4px] overflow-y-auto">
                  {changes.files.slice(0, 60).map((file) => (
                    <div
                      className="git-changes-file hover:bg-[var(--accent-soft)] [&_>_span]:overflow-hidden [&_>_span]:text-[var(--text-secondary)] [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap grid grid-cols-[26px_minmax(0,1fr)] items-center gap-[6px] rounded-[var(--r-sm)] [padding:3px_5px] text-[11px]"
                      key={`${file.status}-${file.path}`}
                    >
                      <code className="text-[var(--star-strong)] text-[10px] font-[700]">
                        {file.status}
                      </code>
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
                <p className="[margin:4px_7px] text-[var(--text-muted)] text-[11px]">
                  {t('chat:focusSession.gitNoChanges')}
                </p>
              )}

              {hasChanges && (
                <div className="git-changes-commit-row [&_input]:w-full [&_input]:min-w-0 [&_input]:[border:1px_solid_var(--stroke)] [&_input]:rounded-[var(--r-sm)] [&_input]:bg-[var(--surface-muted)] [&_input]:p-[5px_7px] [&_input]:text-inherit [&_input]:text-[12px] [&_input:focus]:border-[var(--focus)] [&_input:focus]:[outline:none] grid grid-cols-[minmax(0,1fr)_auto] gap-[6px] [margin:4px_7px]">
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
                  <Button
                    type="button"
                    disabled={busy || streaming}
                    title={
                      streaming
                        ? t('chat:focusSession.gitWaitForRunToFinish')
                        : t('chat:focusSession.gitCommit')
                    }
                    onClick={() => void runAction('commit')}
                  >
                    {running === 'commit' ? (
                      <RefreshCw className="animate-spin" size={12} />
                    ) : (
                      <GitCommitHorizontal size={12} />
                    )}
                    {t('chat:focusSession.gitCommit')}
                  </Button>
                </div>
              )}

              {hasChanges && (
                <button
                  type="button"
                  className="git-changes-view-diff hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] flex w-[calc(100%_-_14px)] min-h-[30px] items-center gap-[7px] [margin:4px_7px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--surface-muted)] [padding:5px_8px] text-[var(--text-secondary)] text-[11px] font-[600] cursor-pointer"
                  onClick={() => {
                    setDiffOpen(true)
                    setOpen(false)
                  }}
                >
                  <FileDiff size={13} />
                  {t('chat:focusSession.gitViewDiff')}
                </button>
              )}

              <div className="flex flex-wrap gap-[6px] [margin:5px_7px_6px]">
                {!isSvn && (
                  <Button
                    type="button"
                    variant="outline"
                    className="bg-surface-subtle"
                    disabled={busy || (!hasCommitsToPush && !hasChanges)}
                    title={
                      hasCommitsToPush
                        ? t('chat:focusSession.gitPushAhead', { count: changes?.ahead || 0 })
                        : t('chat:focusSession.gitPush')
                    }
                    onClick={() => void runAction('push')}
                  >
                    {running === 'push' ? (
                      <RefreshCw className="animate-spin" size={12} />
                    ) : (
                      <Upload size={12} />
                    )}
                    {t('chat:focusSession.gitPush')}
                    {hasCommitsToPush ? ` ↑${changes?.ahead}` : ''}
                  </Button>
                )}
                {confirmingRevert ? (
                  <>
                    <Button
                      type="button"
                      variant="destructive"
                      disabled={busy || streaming}
                      title={
                        streaming
                          ? t('chat:focusSession.gitWaitForRunToFinish')
                          : t('chat:focusSession.gitConfirmRevert')
                      }
                      onClick={() => void runAction('revert')}
                    >
                      {running === 'revert' ? (
                        <RefreshCw className="animate-spin" size={12} />
                      ) : (
                        <Check size={12} />
                      )}
                      {t('chat:focusSession.gitConfirmRevert')}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      className="bg-surface-subtle"
                      disabled={busy}
                      onClick={() => setConfirmingRevert(false)}
                    >
                      <X size={12} />
                      {t('chat:focusSession.gitCancel')}
                    </Button>
                  </>
                ) : (
                  <Button
                    type="button"
                    variant="outline"
                    className="bg-surface-subtle"
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
                  </Button>
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
