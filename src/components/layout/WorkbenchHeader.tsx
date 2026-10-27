import type { ReactNode } from 'react'
import { flushSync } from 'react-dom'
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'

export function WorkbenchHeader({
  title,
  sidebarOpen,
  onToggleSidebar,
  toggleLabel,
  actions,
}: {
  title: ReactNode
  sidebarOpen: boolean
  onToggleSidebar: () => void
  toggleLabel: string
  actions?: ReactNode
}) {
  const { isMobile, persistent } = useSidebar()
  // 展开时开关归侧栏标题所有；收起后在同一顶部位置恢复入口，不占第二行。
  if (persistent || (sidebarOpen && !isMobile)) return null
  return (
    <header
      data-window-drag-region
      className="absolute left-1 top-1 z-30 flex h-11 items-center gap-2"
    >
      <Button
        id="workbench-sidebar-toggle"
        variant="ghost"
        className="size-11 shrink-0 [-webkit-app-region:no-drag]"
        aria-label={toggleLabel}
        title={toggleLabel}
        aria-expanded={sidebarOpen}
        aria-controls="workbench-sidebar"
        onClick={() => {
          // 桌面开关随侧栏迁移，保留键盘操作位置；移动端交给当前导航容器管理焦点。
          if (isMobile) onToggleSidebar()
          else {
            flushSync(onToggleSidebar)
            document.getElementById('workbench-sidebar-toggle')?.focus()
          }
        }}
      >
        {sidebarOpen ? <PanelLeftClose /> : <PanelLeftOpen />}
      </Button>
      <div className="sr-only">{title}</div>
      {actions && (
        <div className="flex shrink-0 items-center gap-1 [-webkit-app-region:no-drag]">
          {actions}
        </div>
      )}
    </header>
  )
}
