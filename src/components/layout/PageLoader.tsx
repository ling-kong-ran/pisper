// 路由懒加载时的过渡占位页。
import { RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'

import { AppEmptyState } from '@/components/ui/app-primitives'

export function PageLoader() {
  const { t } = useI18n()

  return (
    <AppEmptyState>
      <RefreshCw className="animate-spin" size={24} />
      <h2>{t('navigation:pageLoader.lightingUpThisPage')}</h2>
      <p>{t('navigation:pageLoader.bringingTheNeededCapabilitiesIntoPlace')}</p>
    </AppEmptyState>
  )
}
