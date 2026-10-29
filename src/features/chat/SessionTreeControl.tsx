// 会话树开关：会话列表与树形视图的切换控件。
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { TreePine } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { chatApi } from './chat-api'
import { SessionTreeDialog } from './SessionTreeDialog'

import { DropdownMenuItem } from '@/components/ui/dropdown-menu'

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
  children,
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
  children: (menuItem: ReactNode) => ReactNode
}) {
  const { t } = useI18n()
  const [branches, setBranches] = useState(0)
  const [pulsing, setPulsing] = useState(false)
  const openFrame = useRef(0)
  useEffect(() => () => cancelAnimationFrame(openFrame.current), [])

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

  const indicator = branches > 0 && (
    <i aria-hidden="true" className="ml-auto size-1.5 rounded-full bg-star" />
  )
  const menuItem = visible ? (
    <DropdownMenuItem
      aria-label={t('chat:sessionTree.menu')}
      aria-haspopup="dialog"
      data-pisper-recall-pulse={pulsing || undefined}
      onSelect={() => {
        // 先让菜单完成关闭与焦点恢复，再由独立挂载的对话框接管焦点。
        cancelAnimationFrame(openFrame.current)
        openFrame.current = requestAnimationFrame(() => onOpenChange(true))
      }}
    >
      <TreePine
        className={
          pulsing
            ? 'motion-reduce:animate-none [animation:session-tree-new-node-pulse_1.8s_var(--ease-out)]'
            : undefined
        }
      />
      {t('chat:sessionTree.menu')}
      {indicator}
    </DropdownMenuItem>
  ) : null
  return (
    <>
      {children(menuItem)}
      {visible && (
        <SessionTreeDialog
          open={open}
          sessionId={sessionId}
          streaming={streaming}
          onClose={() => onOpenChange(false)}
          onNavigated={onNavigated}
          onCreateChildSession={onCreateChildSession}
        />
      )}
    </>
  )
}
