import { PageFrame } from '@/components/app/PageFrame'
// 配置页：按分区（模型/通知/界面/快捷键/更新/关于）组织设置卡片，
// 每个分区一个设置组件，共享设置原语（SettingsCard 等）。/config 下的远程访问、手机服务器、
// 桌面宠物是功能页，交给 DeviceFeaturePage（功能页模板）。
// 内容根带 data-config-card="section" 锚点，供设置搜索结果跳转高亮定位。
import type { ReactNode } from 'react'
import { AboutSettings } from './AboutSettings'
import { CONFIG_SECTION_ANCHOR, useConfigCardHighlight } from './config-search'
import { DeviceFeaturePage } from './DeviceFeaturePage'
import { InterfaceSettings } from './InterfaceSettings'
import { ShortcutSettings } from './ShortcutSettings'
import { ModelsSettings } from './ModelsSettings'
import { NotificationSettings } from './NotificationSettings'
import { UpdateSettings } from './UpdateSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { NotificationSettingsData } from '@/types/notifications'
import type { AppUpdateController } from '@/types/update'

type ConfigPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  section: string
  // 远程访问 / 手机服务器 / 桌面宠物是功能页：从功能进入时由应用壳给出“‹ 功能”（规格 2.4）。
  onBackToFeatures?: () => void
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  update: AppUpdateController
  renderInterfaceSettings?: (appearance: ReactNode) => ReactNode
}

export function ConfigPage({
  notify,
  registerPrimaryAction,
  section,
  onBackToFeatures,
  onBrowserNotificationChange,
  requestConfirm,
  update,
  renderInterfaceSettings,
}: ConfigPageProps) {
  // 分区切换后消费搜索跳转的高亮请求（同分区点击由事件即时触发）。
  const searchTarget = useConfigCardHighlight(section)
  // 功能 › 设备与连接的三个分区不是设置：用功能页模板，不套设置的 PageFrame（规格 2.4、A6 第 3 批）。
  if (section === 'remote-access' || section === 'mobile-server' || section === 'desktop-pet')
    return (
      <DeviceFeaturePage
        section={section}
        notify={notify}
        requestConfirm={requestConfirm}
        onBack={onBackToFeatures}
      />
    )
  let content
  if (section === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (section === 'interface') {
    const appearance = (
      <InterfaceSettings
        notify={notify}
        defaultAdvancedExpanded={searchTarget?.expandGroup === 'interface-advanced'}
        navigationKey={searchTarget?.navigationKey}
      />
    )
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (section === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (section === 'about') {
    content = <AboutSettings update={update} notify={notify} />
  } else {
    content = (
      <ModelsSettings
        defaultAdvancedExpanded={searchTarget?.expandGroup === 'models-advanced'}
        navigationKey={searchTarget?.navigationKey}
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={requestConfirm}
      />
    )
  }

  return (
    <PageFrame variant={section === 'models' ? 'settings-wide' : 'settings'}>
      <div data-config-card={CONFIG_SECTION_ANCHOR}>{content}</div>
    </PageFrame>
  )
}
