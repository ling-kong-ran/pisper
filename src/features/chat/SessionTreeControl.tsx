// 会话树抽屉宿主：ZCode 式精简后不再渲染悬浮入口（原「追忆」胶囊按钮），
// 树视图统一经会话操作菜单（⋯ → 追忆）打开；这里只挂载对话框本身。
import { SessionTreeDialog } from './SessionTreeDialog'

export function SessionTreeControl({
  open,
  sessionId,
  streaming,
  onOpenChange,
  onNavigated,
  onCreateChildSession,
}: {
  open: boolean
  sessionId: string
  streaming: boolean
  onOpenChange: (open: boolean) => void
  onNavigated: (editorText: string | null) => Promise<void> | void
  onCreateChildSession: (boundaryEntryId: string) => Promise<void> | void
}) {
  return (
    <SessionTreeDialog
      open={open}
      sessionId={sessionId}
      streaming={streaming}
      onClose={() => onOpenChange(false)}
      onNavigated={onNavigated}
      onCreateChildSession={onCreateChildSession}
    />
  )
}
