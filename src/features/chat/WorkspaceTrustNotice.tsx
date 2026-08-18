// 工作区信任提示：首次进入未信任工作区时的安全确认横幅，
// 引导用户理解 Agent 在该目录的执行权限。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Check, RefreshCw, ShieldAlert } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { chatApi, type WorkspaceTrustStatus } from './chat-api'
import { chatErrorMessage } from './chat-errors'

import { Button } from '@/components/ui/button'

export function WorkspaceTrustNotice({
  sessionId,
  cwd,
  streaming = false,
}: {
  sessionId: string
  cwd?: string
  streaming?: boolean
}) {
  const { t } = useI18n()
  const [status, setStatus] = useState<WorkspaceTrustStatus | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [confirmOpen, setConfirmOpen] = useState(false)
  const requestRef = useRef(0)

  const load = useCallback(async () => {
    const request = ++requestRef.current
    setError('')
    try {
      const next = await chatApi.getWorkspaceTrust(sessionId)
      if (request === requestRef.current) setStatus(next)
    } catch (caught) {
      if (request === requestRef.current) setError(chatErrorMessage(caught))
    }
  }, [sessionId])

  useEffect(() => {
    setStatus(null)
    void load()
    return () => {
      requestRef.current += 1
    }
    // The resolved cwd is part of the trust scope, even though the API is session-based.
  }, [cwd, load])

  const decide = async (trusted: boolean) => {
    if (busy || streaming) return
    setBusy(true)
    setError('')
    try {
      setStatus(await chatApi.setWorkspaceTrust(sessionId, trusted))
    } catch (caught) {
      setError(chatErrorMessage(caught))
    } finally {
      setBusy(false)
    }
  }

  if (!status) {
    if (!error) return null
    return (
      <div
        className="tool-approval [&_>_div:first-child]:flex [&_>_div:first-child]:min-w-0 [&_>_div:first-child]:items-center [&_>_div:first-child]:gap-[8px] [&_>_div:first-child_>_span]:flex [&_>_div:first-child_>_span]:min-w-0 [&_>_div:first-child_>_span]:flex-col [&_>_div:first-child_>_span]:gap-[2px] [&_strong]:text-[13px] [&_small]:overflow-hidden [&_small]:text-[13px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_details]:min-w-0 [&_details]:text-[13px] [&_summary]:cursor-pointer [&_pre]:max-h-[130px] [&_pre]:overflow-auto [&_pre]:mt-[6px] [&_pre]:rounded-[var(--r-xs)] [&_pre]:bg-[var(--warning-code-bg)] [&_pre]:text-[var(--warning-soft)] [&_pre]:p-[8px] [&_pre]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_pre]:text-[13px] [&_pre]:whitespace-pre-wrap [&.compact]:grid-cols-[minmax(0,1fr)_auto] [&.compact]:p-[6px_7px] [&.compact_strong]:text-[13px] [&.compact_small]:text-[13px] grid grid-cols-[minmax(0,1fr)_auto] items-center gap-[8px] [border:1px_solid_var(--warning-border)] rounded-[var(--r-sm)] bg-[var(--warning-soft)] text-[var(--warning-strong)] [padding:9px_10px] compact workspace-trust-notice"
        role="alert"
      >
        <div>
          <ShieldAlert size={16} />
          <span>{t('chat:focusSession.workspaceTrustLoadFailed')}</span>
        </div>
        <div className="flex gap-[5px]">
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={() => void load()}
          >
            <RefreshCw size={12} />
            {t('chat:focusSession.retryWorkspaceTrust')}
          </Button>
        </div>
      </div>
    )
  }
  if (!status.resources.length || status.trusted) return null

  return (
    <>
      <div
        className="tool-approval [&_>_div:first-child]:flex [&_>_div:first-child]:min-w-0 [&_>_div:first-child]:items-center [&_>_div:first-child]:gap-[8px] [&_>_div:first-child_>_span]:flex [&_>_div:first-child_>_span]:min-w-0 [&_>_div:first-child_>_span]:flex-col [&_>_div:first-child_>_span]:gap-[2px] [&_strong]:text-[13px] [&_small]:overflow-hidden [&_small]:text-[13px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_details]:min-w-0 [&_details]:text-[13px] [&_summary]:cursor-pointer [&_pre]:max-h-[130px] [&_pre]:overflow-auto [&_pre]:mt-[6px] [&_pre]:rounded-[var(--r-xs)] [&_pre]:bg-[var(--warning-code-bg)] [&_pre]:text-[var(--warning-soft)] [&_pre]:p-[8px] [&_pre]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] [&_pre]:text-[13px] [&_pre]:whitespace-pre-wrap [&.compact]:grid-cols-[minmax(0,1fr)_auto] [&.compact]:p-[6px_7px] [&.compact_strong]:text-[13px] [&.compact_small]:text-[13px] grid grid-cols-[minmax(0,1fr)_auto] items-center gap-[8px] [border:1px_solid_var(--warning-border)] rounded-[var(--r-sm)] bg-[var(--warning-soft)] text-[var(--warning-strong)] [padding:9px_10px] compact workspace-trust-notice"
        role={status.requiresDecision ? 'alert' : 'status'}
        data-pisper-workspace-trust={status.decision === false ? 'restricted' : 'pending'}
      >
        <div>
          <ShieldAlert size={17} />
          <span>
            <strong>
              {status.requiresDecision
                ? t('chat:focusSession.workspaceTrustRequired')
                : t('chat:focusSession.workspaceResourcesRestricted')}
            </strong>
            <small>
              {status.requiresDecision
                ? t('chat:focusSession.workspaceTrustRequiredDescription')
                : t('chat:focusSession.workspaceResourcesRestrictedDescription')}
            </small>
            {error && <small>{error}</small>}
          </span>
        </div>
        <div className="flex gap-[5px]">
          {status.requiresDecision && (
            <Button
              type="button"
              variant="outline"
              className="bg-surface-subtle"
              disabled={busy || streaming}
              onClick={() => void decide(false)}
            >
              {t('chat:focusSession.keepWorkspaceRestricted')}
            </Button>
          )}
          <Button type="button" disabled={busy || streaming} onClick={() => setConfirmOpen(true)}>
            {busy ? <RefreshCw className="animate-spin" size={12} /> : <Check size={12} />}
            {t('chat:focusSession.trustWorkspace')}
          </Button>
        </div>
      </div>
      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogMedia>
              <ShieldAlert />
            </AlertDialogMedia>
            <AlertDialogTitle>{t('chat:focusSession.trustWorkspaceTitle')}</AlertDialogTitle>
            <AlertDialogDescription>
              {t('chat:focusSession.trustWorkspaceDescription')}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('chat:focusSession.gitCancel')}</AlertDialogCancel>
            <AlertDialogAction
              className="text-[var(--primary-foreground)]"
              onClick={() => void decide(true)}
            >
              {t('chat:focusSession.trustWorkspace')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
