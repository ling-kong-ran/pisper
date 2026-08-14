import { useEffect, useState } from 'react'
import { GitBranch } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { chatApi } from './chat-api'
import { SessionTreeDialog } from './SessionTreeDialog'

export function SessionTreeControl({
  visible,
  open,
  sessionId,
  streaming,
  revision,
  onOpenChange,
  onNavigated,
}: {
  visible: boolean
  open: boolean
  sessionId: string
  streaming: boolean
  revision?: number
  onOpenChange: (open: boolean) => void
  onNavigated: (editorText: string | null) => Promise<void> | void
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
      <button
        type="button"
        className="icon-button focus-session-tree-entry"
        title={t('chat:sessionTree.menuDescription')}
        aria-label={t('chat:sessionTree.menu')}
        onClick={() => onOpenChange(true)}
      >
        <GitBranch size={16} />
        {branches > 0 && <i>{branches}</i>}
      </button>
      <SessionTreeDialog
        open={open}
        sessionId={sessionId}
        streaming={streaming}
        onClose={() => onOpenChange(false)}
        onNavigated={onNavigated}
      />
    </>
  )
}
