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
      <div className="tool-approval compact workspace-trust-notice" role="alert">
        <div>
          <ShieldAlert size={16} />
          <span>{t('chat:focusSession.workspaceTrustLoadFailed')}</span>
        </div>
        <div className="tool-approval-actions">
          <button type="button" className="button secondary" onClick={() => void load()}>
            <RefreshCw size={12} />
            {t('chat:focusSession.retryWorkspaceTrust')}
          </button>
        </div>
      </div>
    )
  }
  if (!status.resources.length || status.trusted) return null

  return (
    <>
      <div
        className="tool-approval compact workspace-trust-notice"
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
        <div className="tool-approval-actions">
          {status.requiresDecision && (
            <button
              type="button"
              className="button secondary tiny"
              disabled={busy || streaming}
              onClick={() => void decide(false)}
            >
              {t('chat:focusSession.keepWorkspaceRestricted')}
            </button>
          )}
          <button
            type="button"
            className="button primary tiny"
            disabled={busy || streaming}
            onClick={() => setConfirmOpen(true)}
          >
            {busy ? <RefreshCw className="spin" size={12} /> : <Check size={12} />}
            {t('chat:focusSession.trustWorkspace')}
          </button>
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
            <AlertDialogAction onClick={() => void decide(true)}>
              {t('chat:focusSession.trustWorkspace')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
