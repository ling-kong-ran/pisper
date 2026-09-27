import { PanelLeftClose, PanelLeftOpen } from 'lucide-react'
import { SidebarTrigger, useSidebar } from '@/components/ui/sidebar'
import { useI18n } from '@/app/use-i18n'
import { cn } from '@/lib/utils'

// 展开与收起使用独立按钮，品牌标识不再承担隐含的导航操作。
export function WorkbenchSidebarToggle({ inSidebar = false }: { inSidebar?: boolean }) {
  const { open, openMobile, isMobile } = useSidebar()
  const { t } = useI18n()
  const expanded = isMobile ? openMobile : open
  // 桌面始终保留侧栏内的同一个按钮，折叠后键盘焦点无需跨容器转移。
  if (!isMobile && !inSidebar) return null
  const label = expanded
    ? t('navigation:appSidebar.collapseSidebar')
    : t('navigation:appSidebar.expandSidebar')
  return (
    <SidebarTrigger
      id={inSidebar && expanded ? 'workbench-sidebar-collapse' : 'workbench-sidebar-expand'}
      aria-label={label}
      aria-expanded={expanded}
      aria-controls="workbench-sidebar"
      title={label}
      className={cn(
        'h-8 shrink-0 rounded-lg border-sidebar-border hover:bg-sidebar-accent [-webkit-app-region:no-drag]',
        inSidebar ? (expanded ? 'w-auto gap-1.5 px-2 text-xs' : 'h-11 w-full') : 'w-8',
      )}
    >
      {expanded ? <PanelLeftClose aria-hidden="true" /> : <PanelLeftOpen aria-hidden="true" />}
      {inSidebar && expanded && <span>{t('navigation:workbench.collapseSidebar')}</span>}
    </SidebarTrigger>
  )
}
