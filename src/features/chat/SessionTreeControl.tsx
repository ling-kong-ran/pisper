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
  pulseToken,
  onOpenChange,
  onNavigated,
  onCreateChildSession,
}: {
  visible: boolean
  open: boolean
  sessionId: string
  streaming: boolean
  revision?: number
  pulseToken?: number
  onOpenChange: (open: boolean) => void
  onNavigated: (editorText: string | null) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
}) {
  const { t } = useI18n()
  const [branches, setBranches] = useState(0)
  const [pulsing, setPulsing] = useState(false)

  useEffect(() => {
    if (!pulseToken) return
    setPulsing(true)
    const timeout = window.setTimeout(() => setPulsing(false), 1800)
    return () => window.clearTimeout(timeout)
  }, [pulseToken])

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
        className={`focus-session-tree-entry relative !size-8 !min-w-8 rounded-md border-0 bg-transparent p-0 text-muted-foreground shadow-none hover:bg-muted hover:text-foreground motion-reduce:animate-none ${pulsing ? '[animation:session-tree-new-node-pulse_1.8s_var(--ease-out)]' : ''}`}
        title={t('chat:sessionTree.menuDescription')}
        data-pisper-recall-pulse={pulsing || undefined}
        aria-label={t('chat:sessionTree.menu')}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => onOpenChange(true)}
      >
        <TreePine size={16} />

        {branches > 0 && (
          <i
            aria-hidden="true"
            className="absolute right-1 top-1 size-1 rounded-full bg-blue-500"
          />
        )}
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
