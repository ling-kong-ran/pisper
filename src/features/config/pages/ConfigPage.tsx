// 配置页：按分区（模型/通知/界面/桌面宠物/更新/运行时等）组织设置卡片，
// 每个分区一个设置组件，共享设置原语（SettingsCard 等）。
// 内容根带 data-config-card="section" 锚点，供设置搜索结果跳转高亮定位。
import type { ReactNode } from 'react'
import { AboutSettings } from '@/features/config/components/settings/AboutSettings'
import { CONFIG_SECTION_ANCHOR, useConfigCardHighlight } from '@/features/config/model/config-search'
import { DesktopPetSettings } from '@/features/config/components/settings/DesktopPetSettings'
import { InterfaceSettings } from '@/features/config/components/settings/InterfaceSettings'
import { ShortcutSettings } from '@/features/config/components/settings/ShortcutSettings'
import { ModelsSettings } from '@/features/config/components/settings/ModelsSettings'
import { MobileServerSettings } from '@/features/config/components/mobile/MobileServerSettings'
import { NotificationSettings } from '@/features/config/components/notifications/NotificationSettings'
import { RemoteAccessSettings } from '@/features/config/components/settings/RemoteAccessSettings'
import { RemoteWorkspaceSettings } from '@/features/config/components/settings/RemoteWorkspaceSettings'
import { UpdateSettings } from '@/features/config/components/settings/UpdateSettings'
import type { Notify } from '@/app/routes/route-context'
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
  renderInterfaceSettings?: (appearance: ReactNode) => ReactNode
}

export function ConfigPage({
  notify,
  registerPrimaryAction,
  section,
  onBrowserNotificationChange,
  requestConfirm,
  update,
  renderInterfaceSettings,
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
    const appearance = <InterfaceSettings notify={notify} />
    content = renderInterfaceSettings ? renderInterfaceSettings(appearance) : appearance
  } else if (section === 'shortcuts') {
    content = <ShortcutSettings requestConfirm={requestConfirm} />
  } else if (section === 'desktop-pet') {
    content = <DesktopPetSettings notify={notify} requestConfirm={requestConfirm} />
  } else if (section === 'mobile-server') {
    content = <MobileServerSettings requestConfirm={requestConfirm} />
  } else if (section === 'remote-access') {
    content = (
      <div className="flex flex-col gap-4">
        <RemoteWorkspaceSettings requestConfirm={requestConfirm} />
        {!window.__PISPER_REMOTE_WORKSPACE__ && <RemoteAccessSettings notify={notify} />}
      </div>
    )
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
  } else if (section === 'about') {
    content = <AboutSettings update={update} notify={notify} />
  } else {
    content = (
      <ModelsSettings
        notify={notify}
        registerPrimaryAction={registerPrimaryAction}
        requestConfirm={requestConfirm}
      />
    )
  }

  return (
    <div
      data-config-card={CONFIG_SECTION_ANCHOR}
      className={section === 'models' ? 'mx-auto w-full max-w-[1040px]' : undefined}
    >
      {content}
    </div>
  )
}
