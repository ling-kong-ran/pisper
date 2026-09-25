import { useEffect, useRef, useState } from 'react'
import { PanelRightOpen } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { Plan } from '@/types/chat'
import { SessionContextPanel, type SessionContextTab } from '@/features/chat/SessionContextPanel'
import {
  shouldRevealSessionContext,
  type SessionContextRun,
} from '@/features/chat/session-context-layout'

export function CanvasSessionContext({
  panelId,
  sessionId,
  plan,
  streaming,
  plansAvailable,
  requestConfirm,
  open,
  autoOpen,
  completed,
  onOpenChange,
}: {
  panelId: string
  sessionId: string
  plan: Plan | null
  streaming: boolean
  plansAvailable: boolean
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  open: boolean
  autoOpen: boolean
  completed: boolean
  onOpenChange: (open: boolean) => void
}) {
  const { t } = useI18n()
  const [tab, setTab] = useState<SessionContextTab>('files')
  const previousRun = useRef<SessionContextRun | null>(null)
  useEffect(() => {
    const current = { sessionId, streaming, completed }
    if (autoOpen && shouldRevealSessionContext(previousRun.current, current)) {
      setTab('files')
      onOpenChange(true)
    }
    previousRun.current = current
  }, [autoOpen, completed, onOpenChange, sessionId, streaming])
  if (!open) {
    return (
      <div id={panelId} tabIndex={-1} className="flex min-h-11 flex-1 items-start justify-start">
        <button
          type="button"
          onClick={() => onOpenChange(true)}
          className="flex min-h-11 items-center gap-2 rounded-md px-3 text-sm text-[var(--text-secondary)] hover:bg-[var(--surface-hover)]"
          aria-label={t('chat:sessionContext.open')}
        >
          <PanelRightOpen size={16} />
          {t('chat:sessionContext.title')}
        </button>
      </div>
    )
  }
  return (
    <SessionContextPanel
      embedded
      panelId={panelId}
      compact={false}
      sessionId={sessionId}
      tab={tab}
      plan={plan}
      streaming={streaming}
      plansAvailable={plansAvailable}
      requestConfirm={requestConfirm}
      onTabChange={setTab}
      onClose={() => onOpenChange(false)}
    />
  )
}
