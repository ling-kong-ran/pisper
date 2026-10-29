import type { ReactNode } from 'react'
import { flushSync } from 'react-dom'
import { Menu, PanelLeftClose, PanelLeftOpen } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { cn } from '@/lib/utils'

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
  const { isMobile } = useSidebar()
  // 展开时开关归侧栏标题所有；收起后在同一顶部位置恢复入口，不占第二行。
  if (sidebarOpen && !isMobile) return null
  return (
    // 桌面收起侧栏时是 48px 顶栏里的 28px 图标键；抽屉模式（手机/窄窗）保留 44px 触控键，
    // 落在 52px 手机顶栏的垂直中线上。
    <header
      data-window-drag-region
      className={cn(
        'absolute z-30 flex items-center gap-2',
        isMobile ? 'top-1 left-1 h-11' : 'top-0 left-2 h-12',
      )}
    >
      <Button
        id="workbench-sidebar-toggle"
        variant="ghost"
        size={isMobile ? undefined : 'icon'}
        className={cn(
          'shrink-0 text-content-muted [-webkit-app-region:no-drag]',
          isMobile && 'size-11 text-content-secondary',
        )}
        aria-label={toggleLabel}
        title={toggleLabel}
        aria-expanded={sidebarOpen}
        aria-controls="workbench-sidebar"
        onClick={() => {
          // 桌面开关随侧栏迁移，保留键盘操作位置；移动端交给抽屉管理焦点。
          if (isMobile) onToggleSidebar()
          else {
            flushSync(onToggleSidebar)
            document.getElementById('workbench-sidebar-toggle')?.focus()
          }
        }}
      >
        {isMobile ? (
          <Menu className="size-[22px]" />
        ) : sidebarOpen ? (
          <PanelLeftClose />
        ) : (
          <PanelLeftOpen />
        )}
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
