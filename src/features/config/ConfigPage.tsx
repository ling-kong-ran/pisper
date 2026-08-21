// 配置页：按分区（模型/通知/界面/桌面宠物/更新/运行时等）组织设置卡片，
// 每个分区一个设置组件，共享设置原语（SettingsCard 等）。
import { AboutSettings } from './AboutSettings'
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
    content = <DesktopPetSettings notify={notify} />
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

  return content
}
