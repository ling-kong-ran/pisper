import { RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppCard as Panel } from '@/components/ui/app-primitives'

export function PageLoader() {
  const { t } = useI18n()

  return (
    <Panel className="empty-state">
      <RefreshCw className="spin" size={24} />
      <h2>{t('navigation:pageLoader.lightingUpThisPage')}</h2>
      <p>{t('navigation:pageLoader.bringingTheNeededCapabilitiesIntoPlace')}</p>
    </Panel>
  )
}
