import { BrandLogo } from '@/components/BrandLogo'
import { SidebarTrigger, useSidebar } from '@/components/ui/sidebar'
import { useI18n } from '@/app/use-i18n'

// One brand control stays at the left edge, whether navigation is open or closed.
export function WorkbenchSidebarToggle({ inSidebar = false }: { inSidebar?: boolean }) {
  const { open, isMobile } = useSidebar()
  const { t } = useI18n()
  if (!isMobile && inSidebar !== open) return null
  return (
    <SidebarTrigger
      aria-label={t('navigation:workbench.toggleSidebar')}
      title={t('navigation:workbench.toggleSidebar')}
      className="size-8 shrink-0 rounded-lg hover:bg-sidebar-accent [-webkit-app-region:no-drag]"
    >
      <BrandLogo className="size-6" size={24} />
    </SidebarTrigger>
  )
}
