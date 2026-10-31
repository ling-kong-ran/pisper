// 工作室共用的放置名、预览档位、能力名与版本来源文案（文案在懒加载的 custom-ui 命名空间）。
// 单独成文件：组件文件只导出组件（快速刷新），纯函数放这里。
import type { ComponentSurface } from '@shared/custom-ui-manifest.mjs'
import type { ComponentVersion } from '@/features/custom-ui/lifecycle-api'
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

export function capabilityLabel(type: string, t: Translate) {
  switch (type) {
    case 'network':
      return t('custom-ui:studio.capNetwork')
    case 'notify':
      return t('custom-ui:studio.capNotify')
    case 'sessions.read':
      return t('custom-ui:studio.capSessionsRead')
    case 'sessions.open':
      return t('custom-ui:studio.capSessionsOpen')
    default:
      return t('custom-ui:studio.capStorage')
  }
}

export function versionSourceLabel(version: Pick<ComponentVersion, 'source'>, t: Translate) {
  switch (version.source) {
    case 'agent':
      return t('custom-ui:versions.sourceAgent')
    case 'import':
      return t('custom-ui:versions.sourceImport')
    case 'restore':
      return t('custom-ui:versions.sourceRestore')
    case 'local':
      return t('custom-ui:versions.sourceLocal')
    default:
      return ''
  }
}
