// 会话树入口：顶栏 ⑂（规格 v2 第 4.5 节，唯一入口）与旧外壳“⋯”菜单项共用的状态与对话框。
// 分支数来自共享的会话树摘要查询（session-tree-summary），改标记、从此处继续后立即更新。
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { GitFork } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import { useI18n } from '@/app/use-i18n'
import { DropdownMenuItem } from '@/components/ui/dropdown-menu'
import { SessionTreeDialog } from './SessionTreeDialog'
import { useSessionTreeSummary } from './session-tree-summary'
import type { TreeView } from './session-tree-model'

// 会话树对话框的三个页（规格 4.5）：分支 / 标记（本会话）/ 全部标记（所有会话）。
export type SessionTreeView = TreeView

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
  notify,
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
  // 对话框里“从此处继续”、切到另一条分支后的提示（带撤销）。
  notify?: Notify
  // 第二个参数给会话页头用：分支数与按页签打开会话树。
  children: (
    menuItem: ReactNode,
    tree: {
      branches: number
      open: (view?: SessionTreeView, opener?: HTMLElement | null) => void
    },
  ) => ReactNode
}) {
  const { t } = useI18n()
  const summary = useSessionTreeSummary(sessionId, { enabled: visible, streaming, revision })
  const branches = summary?.branchCount ?? 0
  const [initialView, setInitialView] = useState<SessionTreeView>('branches')
  const [pulsing, setPulsing] = useState(false)
  const openFrame = useRef(0)
  // 从页头按钮打开时记住按钮，关闭后焦点回到它；从“⋯”菜单打开时为空，对话框退回到“⋯”。
  const opener = useRef<HTMLElement | null>(null)
  useEffect(() => () => cancelAnimationFrame(openFrame.current), [])

  useEffect(() => {
    if (!pulseToken) return
    setPulsing(true)
    const timeout = window.setTimeout(() => setPulsing(false), 1800)
    return () => window.clearTimeout(timeout)
  }, [pulseToken])

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
        opener.current = null
        setInitialView('branches')
        cancelAnimationFrame(openFrame.current)
        openFrame.current = requestAnimationFrame(() => onOpenChange(true))
      }}
    >
      <GitFork
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
      {children(menuItem, {
        branches,
        open: (view = 'branches', from = null) => {
          opener.current = from
          setInitialView(view)
          cancelAnimationFrame(openFrame.current)
          openFrame.current = requestAnimationFrame(() => onOpenChange(true))
        },
      })}
      {visible && (
        <SessionTreeDialog
          open={open}
          initialView={initialView}
          returnFocusRef={opener}
          sessionId={sessionId}
          streaming={streaming}
          onClose={() => onOpenChange(false)}
          onNavigated={onNavigated}
          onCreateChildSession={onCreateChildSession}
          notify={notify}
        />
      )}
    </>
  )
}
