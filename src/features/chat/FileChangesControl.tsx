// 文件变更审批：无 Git/SVN 工作区也能预览/批准/撤销 Agent 的文件改动。
// 数据源是 runtime 在 edit/write 执行前留下的快照（见 session-file-changes.mjs）。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Check, FileDiff, ListChecks, RefreshCw, Undo2, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { chatApi, type SessionFileChangesResponse } from './chat-api'
import { AnchoredPopupMenu } from './AnchoredPopupMenu'
import { GitDiffDialog } from './GitDiffViewer'
import { invalidateSessionChangeSummary } from './session-change-summary-api'

import { Button } from '@/components/ui/button'

function isError(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

type RunningAction = 'approve-all' | 'revert-all'

const STATUS_LABEL: Record<string, string> = { modified: 'M', created: 'A', deleted: 'D' }

export function FileChangesControl({
  sessionId,
  streaming,
}: {
  sessionId?: string
  streaming?: boolean
}) {
  const { t } = useI18n()
  const [open, setOpen] = useState(false)
  const [diff, setDiff] = useState<{ diff: string; truncated: boolean; path: string } | null>(null)
  const [loading, setLoading] = useState(false)
  const [running, setRunning] = useState<RunningAction | null>(null)
  const [confirmingRevertAll, setConfirmingRevertAll] = useState(false)
  // 单文件撤销的两段确认：path → 是否处于待确认态。
  const [confirmingRevert, setConfirmingRevert] = useState<string | null>(null)
  const [changes, setChanges] = useState<SessionFileChangesResponse | null>(null)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const rootRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const menuRef = useRef<HTMLDivElement>(null)

  const closeMenu = useCallback((restoreFocus = false) => {
    setOpen(false)
    setConfirmingRevert(null)
    setConfirmingRevertAll(false)
    if (restoreFocus) triggerRef.current?.focus()
  }, [])

  const load = useCallback(async () => {
    if (!sessionId) return
    setLoading(true)
    setError('')
    try {
      setChanges(await chatApi.getSessionFileChanges(sessionId))
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
      // 菜单已 portal 到 body，点击菜单内部不算外部点击。
      if (!rootRef.current?.contains(target) && !menuRef.current?.contains(target)) closeMenu()
    }
    document.addEventListener('mousedown', close)
    return () => document.removeEventListener('mousedown', close)
  }, [open, load, closeMenu])

  // 运行结束后刷新待审批数量，徽标保持最新；面板打开时也随流式状态刷新。
  useEffect(() => {
    if (!streaming) void load().catch(() => {})
  }, [streaming, load])

  // 查看单文件 diff：审批（审核）的入口。
  const viewDiff = async (path: string) => {
    if (!sessionId) return
    setError('')
    try {
      const result = await chatApi.getSessionFileChangeDiff(sessionId, path)
      if (!result.diff?.trim()) {
        setError(t('chat:focusSession.fileChangesNoSnapshot'))
        return
      }
      setDiff({ diff: result.diff, truncated: Boolean(result.diffTruncated), path })
    } catch (caught) {
      setError(isError(caught))
    }
  }

  const revertFile = async (path: string) => {
    if (!sessionId || confirmingRevert !== path) {
      setConfirmingRevert(path)
      return
    }
    setConfirmingRevert(null)
    setError('')
    setNotice('')
    try {
      const result = await chatApi.revertSessionFileChanges(sessionId, path)
      setChanges(result)
      void invalidateSessionChangeSummary(sessionId)
      setNotice(
        result.reverted
          ? t('chat:focusSession.fileChangesRevertDone')
          : t('chat:focusSession.fileChangesRevertUnavailable'),
      )
    } catch (caught) {
      setError(isError(caught))
    }
  }

  const approveFile = async (path: string) => {
    if (!sessionId) return
    setError('')
    try {
      const result = await chatApi.approveSessionFileChanges(sessionId, path)
      setChanges(result)
      void invalidateSessionChangeSummary(sessionId)
    } catch (caught) {
      setError(isError(caught))
    }
  }

  const runAction = async (action: RunningAction) => {
    if (!sessionId || running) return
    if (action === 'revert-all' && !confirmingRevertAll) {
      setConfirmingRevertAll(true)
      return
    }
    setConfirmingRevertAll(false)
    setRunning(action)
    setError('')
    setNotice('')
    try {
      const result =
        action === 'approve-all'
          ? await chatApi.approveSessionFileChanges(sessionId)
          : await chatApi.revertSessionFileChanges(sessionId)
      setChanges(result)
      void invalidateSessionChangeSummary(sessionId)
      if (action === 'approve-all') {
        setNotice(t('chat:focusSession.fileChangesApproveDone'))
      } else if (!result.reverted) {
        setNotice(t('chat:focusSession.fileChangesRevertUnavailable'))
      } else if (result.files.some((file) => file.pending)) {
        setNotice(t('chat:focusSession.fileChangesRevertPartial', { count: result.reverted }))
      } else {
        setNotice(t('chat:focusSession.fileChangesRevertDone'))
      }
    } catch (caught) {
      setError(isError(caught))
    } finally {
      setRunning(null)
    }
  }

  const pendingCount = changes?.summary.pending || 0
  const hasFiles = Boolean(changes?.summary.files)
  const reversibleCount =
    changes?.files.filter((file) => file.canRevert && !file.reverted).length || 0
  const busy = Boolean(running)
  const label = t('chat:focusSession.fileChanges')

  return (
    <div
      ref={rootRef}
      className={`file-changes-select [.composer-tool-tray_&]:w-[38px] [.composer-tool-tray_&]:min-w-[38px] [.composer-tool-tray_&]:h-[38px] [.composer-tool-tray_&]:flex-none @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:min-w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[700px]:[.composer-tool-tray_&]:p-0 @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:min-w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] relative flex-none w-[38px] h-[38px] text-[var(--text-tertiary)] ${open ? 'open' : ''}    ${pendingCount > 0 ? 'active' : ''}`}
    >
      <button
        ref={triggerRef}
        type="button"
        className="file-changes-trigger hover:border-[var(--accent-border)] hover:bg-[var(--accent-soft)] hover:text-[var(--star-strong)] [.file-changes-select.open_&]:border-[var(--accent-border)] [.file-changes-select.open_&]:bg-[var(--accent-soft)] [.file-changes-select.open_&]:text-[var(--star-strong)] [.file-changes-select.active_&]:text-[var(--star-strong)] [&_>_i]:absolute [&_>_i]:top-[-4px] [&_>_i]:right-[-6px] [&_>_i]:min-w-[15px] [&_>_i]:rounded-[var(--r-pill)] [&_>_i]:bg-[var(--star-strong)] [&_>_i]:p-[1px_4px] [&_>_i]:text-[var(--on-accent)] [&_>_i]:text-[9px] [&_>_i]:[font-style:normal] [&_>_i]:font-[700] [&_>_i]:leading-[1.3] [&_>_i]:text-center @max-[700px]:[.composer-tool-tray_&]:w-[32px] @max-[700px]:[.composer-tool-tray_&]:h-[32px] @max-[470px]:[.composer-tool-tray_&]:w-[28px] @max-[470px]:[.composer-tool-tray_&]:h-[28px] relative grid w-full h-full place-items-center [border:1px_solid_transparent] rounded-[var(--r-sm)] bg-[var(--surface-muted)] text-inherit cursor-pointer"
        title={label}
        aria-label={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={!sessionId}
        onClick={() => (open ? closeMenu() : setOpen(true))}
      >
        <ListChecks size={14} />
        {pendingCount > 0 && <i>{pendingCount > 99 ? '99+' : pendingCount}</i>}
      </button>
      <AnchoredPopupMenu
        open={open}
        anchorRef={rootRef}
        menuRef={menuRef}
        className="anchored-popup-menu file-changes-menu w-[min(340px,calc(100vw_-_28px))] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-md)] bg-[var(--solid)] [padding:5px] shadow-[0_18px_42px_-18px_var(--menu-shadow)]"
        role="dialog"
        ariaLabel={label}
        onClose={() => closeMenu(true)}
      >
        <div className="file-changes-menu-head hover:bg-[var(--accent-soft)] [&_>_span:nth-child(2)]:flex [&_>_span:nth-child(2)]:min-w-0 [&_>_span:nth-child(2)]:flex-col [&_>_span:nth-child(2)]:gap-[2px] [&_strong]:text-[12px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap grid min-h-[44px] grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[8px] rounded-[var(--r-sm)] [padding:6px_7px]">
          <span
            className={`file-changes-menu-icon grid w-[32px] h-[32px] place-items-center rounded-[var(--r-sm)] ${pendingCount > 0 ? 'bg-[var(--star-soft)] text-[var(--star-strong)]' : 'bg-[var(--surface-muted)] text-[var(--text-muted)]'}`}
          >
            <FileDiff size={15} />
          </span>
          <span>
            <strong>{label}</strong>
            <small>
              {hasFiles
                ? `${t('chat:focusSession.fileChangesCount', { count: changes?.summary.files || 0 })} · +${changes?.summary.added || 0} −${changes?.summary.removed || 0}${pendingCount > 0 ? ` · ${t('chat:focusSession.fileChangesPending', { count: pendingCount })}` : ''}`
                : loading
                  ? t('chat:focusSession.gitLoading')
                  : t('chat:focusSession.fileChangesEmpty')}
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
          <p
            role="alert"
            className="[margin:2px_7px_4px] text-[var(--danger,_#d64545)] text-[11px]"
          >
            {error}
          </p>
        )}
        {notice && !error && (
          <p
            role="status"
            className="[margin:2px_7px_4px] text-[var(--success,_#2e9e63)] text-[11px]"
          >
            {notice}
          </p>
        )}

        {hasFiles && (
          <>
            <div className="file-changes-file-list [&_>_small]:p-[3px_5px] [&_>_small]:text-[var(--text-muted)] flex max-h-[200px] flex-col gap-[1px] [margin:2px_4px] overflow-y-auto">
              {(changes?.files || []).map((file) => (
                <div
                  className="file-changes-file hover:bg-[var(--accent-soft)] [&_>_span]:overflow-hidden [&_>_span]:text-[var(--text-secondary)] [&_>_span]:text-ellipsis [&_>_span]:whitespace-nowrap grid grid-cols-[26px_minmax(0,1fr)_auto] items-center gap-[6px] rounded-[var(--r-sm)] [padding:3px_5px] text-[11px]"
                  key={file.path}
                >
                  <code
                    className={`text-[10px] font-[700] ${file.status === 'created' ? 'text-[var(--success,_#2e9e63)]' : file.status === 'deleted' ? 'text-[var(--danger,_#d64545)]' : 'text-[var(--star-strong)]'}`}
                  >
                    {STATUS_LABEL[file.status] || 'M'}
                  </code>
                  <button
                    type="button"
                    className="min-w-0 overflow-hidden text-left text-ellipsis whitespace-nowrap bg-transparent border-0 p-0 text-inherit cursor-pointer"
                    title={`${file.path} · +${file.added} −${file.removed}`}
                    onClick={() => void viewDiff(file.path)}
                  >
                    <span className="block overflow-hidden text-ellipsis whitespace-nowrap">
                      {file.path}
                    </span>
                    <small className="text-[var(--text-muted)]">
                      +{file.added} −{file.removed}
                      {file.reverted
                        ? ` · ${t('chat:focusSession.fileChangesReverted')}`
                        : file.approved
                          ? ` · ${t('chat:focusSession.fileChangesApproved')}`
                          : ''}
                    </small>
                  </button>
                  <span className="flex flex-none items-center gap-[2px]">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      title={t('chat:focusSession.fileChangesApproveFile')}
                      aria-label={t('chat:focusSession.fileChangesApproveFile')}
                      disabled={busy || streaming || file.approved || file.reverted}
                      onClick={() => void approveFile(file.path)}
                    >
                      <Check size={12} />
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      className={
                        confirmingRevert === file.path ? 'text-[var(--danger,_#d64545)]' : ''
                      }
                      title={t('chat:focusSession.fileChangesRevertFile')}
                      aria-label={
                        confirmingRevert === file.path
                          ? t('chat:focusSession.fileChangesConfirmRevertFile')
                          : t('chat:focusSession.fileChangesRevertFile')
                      }
                      disabled={busy || streaming || file.reverted || !file.canRevert}
                      onClick={() => void revertFile(file.path)}
                    >
                      {confirmingRevert === file.path ? <X size={12} /> : <Undo2 size={12} />}
                    </Button>
                  </span>
                </div>
              ))}
            </div>

            <div className="flex flex-wrap gap-[6px] [margin:5px_7px_6px]">
              <Button
                type="button"
                variant="outline"
                className="bg-surface-subtle"
                disabled={busy || streaming || pendingCount === 0}
                title={
                  streaming
                    ? t('chat:focusSession.gitWaitForRunToFinish')
                    : t('chat:focusSession.fileChangesApproveAll')
                }
                onClick={() => void runAction('approve-all')}
              >
                {running === 'approve-all' ? (
                  <RefreshCw className="animate-spin" size={12} />
                ) : (
                  <Check size={12} />
                )}
                {t('chat:focusSession.fileChangesApproveAll')}
              </Button>
              {confirmingRevertAll ? (
                <>
                  <Button
                    type="button"
                    variant="destructive"
                    disabled={busy || streaming}
                    title={
                      streaming
                        ? t('chat:focusSession.gitWaitForRunToFinish')
                        : t('chat:focusSession.fileChangesConfirmRevert')
                    }
                    onClick={() => void runAction('revert-all')}
                  >
                    {running === 'revert-all' ? (
                      <RefreshCw className="animate-spin" size={12} />
                    ) : (
                      <Undo2 size={12} />
                    )}
                    {t('chat:focusSession.fileChangesConfirmRevert')}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    className="bg-surface-subtle"
                    disabled={busy}
                    onClick={() => setConfirmingRevertAll(false)}
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
                  disabled={busy || streaming || reversibleCount === 0}
                  title={t('chat:focusSession.fileChangesRevertAllDescription')}
                  onClick={() => void runAction('revert-all')}
                >
                  <Undo2 size={12} />
                  {t('chat:focusSession.fileChangesRevertAll')}
                </Button>
              )}
            </div>
          </>
        )}
      </AnchoredPopupMenu>
      {diff && (
        <GitDiffDialog diff={diff.diff} truncated={diff.truncated} onClose={() => setDiff(null)} />
      )}
    </div>
  )
}
