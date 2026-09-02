// 配置页：按分区（模型/通知/界面/桌面宠物/更新/运行时等）组织设置卡片，
// 每个分区一个设置组件，共享设置原语（SettingsCard 等）。
// 内容根带 data-config-card="section" 锚点，供设置搜索结果跳转高亮定位。
import { AboutSettings } from './AboutSettings'
import { CONFIG_SECTION_ANCHOR, useConfigCardHighlight } from './config-search'
import { DesktopPetSettings } from './DesktopPetSettings'
import { InterfaceSettings } from './InterfaceSettings'
import { ModelsSettings } from './ModelsSettings'
import { MobileServerSettings } from './MobileServerSettings'
import { NotificationSettings } from './NotificationSettings'
import { RemoteAccessSettings } from './RemoteAccessSettings'
import { UpdateSettings } from './UpdateSettings'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { NotificationSettingsData } from '@/types/notifications'
import type { AppUpdateController } from '@/types/update'

type ConfigPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  section: string
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  update: AppUpdateController
}

export function ConfigPage({
  notify,
  registerPrimaryAction,
  section,
  onBrowserNotificationChange,
  requestConfirm,
  update,
}: ConfigPageProps) {
  // 分区切换后消费搜索跳转的高亮请求（同分区点击由事件即时触发）。
  useConfigCardHighlight(section)
  let content
  if (section === 'notifications') {
    content = (
      <NotificationSettings
        notify={notify}
        onBrowserNotificationChange={onBrowserNotificationChange}
      />
    )
  } else if (section === 'interface') {
    content = <InterfaceSettings notify={notify} />
  } else if (section === 'desktop-pet') {
    content = <DesktopPetSettings notify={notify} requestConfirm={requestConfirm} />
  } else if (section === 'mobile-server') {
    content = <MobileServerSettings />
  } else if (section === 'remote-access') {
    content = <RemoteAccessSettings notify={notify} />
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (section === 'about') {
    content = <AboutSettings update={update} />
  } else {
    content = (
      <ModelsSettings
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={requestConfirm}
      />
    )
  }

  return <div data-config-card={CONFIG_SECTION_ANCHOR}>{content}</div>
}
