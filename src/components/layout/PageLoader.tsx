import { RefreshCw } from 'lucide-react'
import { useI18n } from '../../app/use-i18n'
import { Panel } from '../ui'

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
