// 全局菜单按需加载，避免非当前页面的图标和菜单原语进入首屏依赖图。
import { Blocks, Settings, FileDiff, TerminalSquare, Activity, Link2, Globe2 } from 'lucide-react'
import { ActivityRail } from '@/components/layout/ActivityRail'
import { MobileTabBar } from '@/components/layout/MobileTabBar'
import { Button } from '@/components/ui/button'
import { useI18n } from '@/app/use-i18n'
import { getActivityNavigation } from '@/app/activity-navigation'
import { SETTINGS_PAGES } from '@/app/settings-navigation'
import { requestTaskContent, taskContentKinds } from '@/features/chat/public-workspace'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

export function WorkbenchNavigation({
  page,
  activeSessionId,
  installedTools,
  onSelect,
  onSettings,
  variant = 'rail',
}: {
  variant?: 'rail' | 'tabbar'
  page: string
  activeSessionId: string
  installedTools: Array<{ id: string; name: string }>
  onSelect: (id: string) => void
  onSettings: () => void
}) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const groups = getActivityNavigation(t, capabilities).map((group) =>
    group.id === 'extensions'
      ? {
          ...group,
          items: [
            ...group.items,
            ...installedTools.map((tool) => ({
              id: `component:${tool.id}`,
              label: tool.name,
              icon: Blocks,
            })),
          ],
        }
      : group.id === 'resources'
        ? {
            ...group,
            items: [
              ...group.items,
              {
                id: 'task:changes',
                label: t('chat:workbench.projectChanges'),
                icon: FileDiff,
                disabled:
                  page !== 'chat' ||
                  !activeSessionId ||
                  !runtimeFeatureAvailable(capabilities, 'vcs'),
              },
              {
                id: 'task:terminal',
                label: t('terminal:workspace.terminal'),
                icon: TerminalSquare,
                disabled:
                  page !== 'chat' ||
                  !activeSessionId ||
                  !window.pisperDesktop?.terminalCreate ||
                  !runtimeFeatureAvailable(capabilities, 'terminal'),
              },
              {
                id: 'task:processes',
                label: t('terminal:workspace.processes'),
                icon: Activity,
                disabled:
                  page !== 'chat' || !activeSessionId || !window.pisperDesktop?.terminalCreate,
              },
              {
                id: 'task:sources',
                label: t('chat:workbench.sources'),
                icon: Link2,
                disabled: page !== 'chat' || !activeSessionId,
              },
              {
                id: 'task:browser',
                label: t('common:webPreview.title'),
                icon: Globe2,
                disabled: page !== 'chat' || !activeSessionId,
              },
            ],
          }
        : group,
  )
  const activeId = page === 'workflowCreate' ? 'workflows' : page
  const select = (id: string) => {
    const kind = taskContentKinds.find((kind) => id === `task:${kind}`)
    if (kind) requestTaskContent(kind)
    else onSelect(id)
  }
  if (variant === 'tabbar') {
    return (
      <MobileTabBar
        label={t('navigation:appSidebar.mainNavigation')}
        groups={groups}
        activeId={activeId}
        onSelect={select}
        shortLabels={{ workspace: t('navigation:rail.workspaceShort') }}
        groupPagesLabel={(group) => t('navigation:rail.groupPages', { group })}
        settings={{
          label: t('navigation:navigation.settings'),
          icon: Settings,
          contains: (id) => SETTINGS_PAGES.has(id),
          onSelect: onSettings,
        }}
      />
    )
  }
  return (
    <ActivityRail
      label={t('navigation:appSidebar.mainNavigation')}
      groups={groups}
      activeId={activeId}
      onSelect={select}
      footer={
        <Button
          variant="ghost"
          className="size-11"
          aria-label={t('navigation:navigation.settings')}
          title={t('navigation:navigation.settings')}
          onClick={onSettings}
        >
          <Settings className="size-5" />
        </Button>
      }
    />
  )
}
