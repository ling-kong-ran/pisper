// 路由懒加载时的过渡占位页：使用品牌轨道指示，与其他运行中状态保持同一视觉语言。
import { useI18n } from '@/app/use-i18n'

import { AppEmptyState } from '@/components/ui/app-primitives'
import { OrbitSpinner } from '@/components/ui/spinner'

export function PageLoader() {
  const { t } = useI18n()

  return (
    <AppEmptyState>
      {/* 包一层避免空状态给首个 svg 套上虚线圆框。 */}
      <span className="grid place-items-center">
        <OrbitSpinner className="size-6" />
      </span>
      <h2>{t('navigation:pageLoader.lightingUpThisPage')}</h2>
      <p>{t('navigation:pageLoader.bringingTheNeededCapabilitiesIntoPlace')}</p>
    </AppEmptyState>
  )
}
