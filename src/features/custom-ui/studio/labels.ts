// 工作室共用的放置名与预览档位文案（文案在懒加载的 custom-ui 命名空间）。
import type { ComponentSurface } from '@shared/custom-ui-manifest.mjs'
import type { PreviewTarget } from './studio-model'

type Translate = (key: string, values?: Record<string, string | number>) => string

export function surfaceLabel(surface: ComponentSurface, t: Translate) {
  if (surface === 'overview-card') return t('custom-ui:studio.surfaceOverview')
  if (surface === 'session-panel') return t('custom-ui:studio.surfaceSession')
  if (surface === 'feature-page') return t('custom-ui:studio.surfaceFeature')
  return t('custom-ui:studio.surfaceFloating')
}

export function targetLabel(target: PreviewTarget, t: Translate) {
  switch (target) {
    case 'overview-S':
      return t('custom-ui:studio.targetOverviewS')
    case 'overview-M':
      return t('custom-ui:studio.targetOverviewM')
    case 'session-panel':
      return t('custom-ui:studio.surfaceSession')
    case 'feature-page':
      return t('custom-ui:studio.surfaceFeature')
    default:
      return t('custom-ui:studio.surfaceFloating')
  }
}
