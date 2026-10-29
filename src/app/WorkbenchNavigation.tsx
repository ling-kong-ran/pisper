// 全局菜单按需加载，避免非当前页面的图标和菜单原语进入首屏依赖图。
import { Blocks, Settings, FileDiff, TerminalSquare, Activity, Link2, Globe2 } from 'lucide-react'
import { ActivityRail, ActivityRailButton } from '@/components/layout/ActivityRail'
import { MobileTabBar } from '@/components/layout/MobileTabBar'
import { useI18n } from '@/app/use-i18n'
import { getActivityNavigation } from '@/app/activity-navigation'
import { requestTaskContent, taskContentKinds } from '@/features/chat/public-workspace'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

export function WorkbenchNavigation({
  page,
  settingsOwned = page === 'config',
  componentId = '',
  activeSessionId,
  installedTools,
  onSelect,
  onSettings,
  variant = 'rail',
}: {
  variant?: 'rail' | 'tabbar'
  page: string
  // 当前路由是否归设置（壳层按进入来源判定）；归设置时只点亮设置键，不再点亮功能分组。
  settingsOwned?: boolean
  // 自定义组件工具页的组件 id，用来点亮扩展分组里对应的已安装工具。
  componentId?: string
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
  // 功能分组与设置对同一路由只认一个归属；内置组件不在已安装工具列表里，退回「组件」项。
  const componentItem = `component:${componentId}`
  const activeId = settingsOwned
    ? ''
    : page === 'workflowCreate'
      ? 'workflows'
      : page === 'customComponentTool'
        ? installedTools.some((tool) => tool.id === componentId)
          ? componentItem
          : 'components'
        : page
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
          active: settingsOwned,
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
      shortLabels={{ workspace: t('navigation:rail.workspaceShort') }}
      footer={
        // 插件、星忆等页同时归属功能分组；按进入来源只点亮一处，避免功能栏出现两个选中刻度。
        <ActivityRailButton
          icon={Settings}
          label={t('navigation:navigation.settings')}
          active={settingsOwned}
          onClick={onSettings}
        />
      }
    />
  )
}
