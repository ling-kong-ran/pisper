import { useI18n } from '@/app/use-i18n'
import { CliSettings } from './CliSettings'
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
  setSection: (section: string) => void
  onBrowserNotificationChange?: (settings: NotificationSettingsData) => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  update: AppUpdateController
}

type ConfigSubnavProps = Pick<ConfigPageProps, 'section' | 'setSection'>

function ConfigSubnav({ section, setSection }: ConfigSubnavProps) {
  const { t } = useI18n()
  return (
    <div className="config-subnav">
      <button className={section === 'models' ? 'active' : ''} onClick={() => setSection('models')}>
        {t('config:configPage.models')}
      </button>
      <button
        className={section === 'notifications' ? 'active' : ''}
        onClick={() => setSection('notifications')}
      >
        {t('config:configPage.notifications')}
      </button>
      <button
        className={section === 'interface' ? 'active' : ''}
        onClick={() => setSection('interface')}
      >
        {t('config:configPage.interface')}
      </button>
      <button
        className={section === 'terminal' ? 'active' : ''}
        onClick={() => setSection('terminal')}
      >
        {t('config:configPage.terminal')}
      </button>
      <button
        className={section === 'desktop-pet' ? 'active' : ''}
        onClick={() => setSection('desktop-pet')}
      >
        {t('config:configPage.desktopPet')}
      </button>
      <button
        className={section === 'updates' ? 'active' : ''}
        onClick={() => setSection('updates')}
      >
        {t('config:configPage.appUpdates')}
      </button>
    </div>
  )
}

export function ConfigPage({
  notify,
  registerPrimaryAction,
  section,
  setSection,
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
  } else if (section === 'terminal') {
    content = (
      <div className="language-settings">
        <CliSettings notify={notify} />
      </div>
    )
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

  return (
    <>
      <ConfigSubnav section={section} setSection={setSection} />
      {content}
    </>
  )
}
