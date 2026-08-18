// 会话树开关：会话列表与树形视图的切换控件。
import { useEffect, useState } from 'react'
import { TreePine } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { chatApi } from './chat-api'
import { SessionTreeDialog } from './SessionTreeDialog'

import { Button } from '@/components/ui/button'

export function SessionTreeControl({
  visible,
  open,
  sessionId,
  streaming,
  revision,
  onOpenChange,
  onNavigated,
  onCreateChildSession,
}: {
  visible: boolean
  open: boolean
  sessionId: string
  streaming: boolean
  revision?: number
  onOpenChange: (open: boolean) => void
  onNavigated: (editorText: string | null) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
}) {
  const { t } = useI18n()
  const [branches, setBranches] = useState(0)

  useEffect(() => {
    if (!visible || streaming) return
    let active = true
    void chatApi
      .getSessionTree(sessionId)
      .then((tree) => active && setBranches(tree.branchCount || 0))
      .catch(() => {})
    return () => {
      active = false
    }
  }, [revision, sessionId, streaming, visible])

  if (!visible) return null
  return (
    <>
      <Button
        type="button"
        variant="ghost"
        size="icon"
        className="focus-session-tree-entry hover:border-[var(--brand-blue-border)] hover:bg-[var(--solid)] hover:text-[var(--brand-blue-strong)] [&_>_span]:text-[11px] [&_>_span]:font-[650] [&_>_span]:whitespace-nowrap [&_i]:grid [&_i]:min-w-[17px] [&_i]:h-[17px] [&_i]:place-items-center [&_i]:rounded-[var(--r-pill)] [&_i]:bg-[var(--brand-blue-soft)] [&_i]:text-[var(--brand-blue-strong)] [&_i]:p-[0_4px] [&_i]:text-[9px] [&_i]:[font-style:normal] [&_i]:[font-variant-numeric:tabular-nums] absolute z-[6] [top:12px] [right:18px] inline-flex w-auto min-w-[32px] h-[32px] items-center gap-[6px] [border:1px_solid_var(--stroke)] rounded-[var(--r-pill)] bg-[color-mix(in_srgb,var(--solid)_90%,transparent)] text-[var(--text-muted)] [padding:0_10px_0_8px] shadow-[var(--sh-1)] [backdrop-filter:blur(10px)]"
        title={t('chat:sessionTree.menuDescription')}
        aria-label={t('chat:sessionTree.menu')}
        onClick={() => onOpenChange(true)}
      >
        <TreePine size={16} />
        <span>{t('chat:sessionTree.menu')}</span>
        {branches > 0 && <i>{branches}</i>}
      </Button>
      <SessionTreeDialog
        open={open}
        sessionId={sessionId}
        streaming={streaming}
        onClose={() => onOpenChange(false)}
        onNavigated={onNavigated}
        onCreateChildSession={onCreateChildSession}
      />
    </>
  )
}
