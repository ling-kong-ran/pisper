import { DesktopPetSettings } from './DesktopPetSettings'
import { LanguageSettings } from './LanguageSettings'
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
    content = <LanguageSettings notify={notify} />
  } else if (section === 'desktop-pet') {
    content = <DesktopPetSettings notify={notify} />
  } else if (section === 'updates') {
    content = <UpdateSettings notify={notify} update={update} />
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
