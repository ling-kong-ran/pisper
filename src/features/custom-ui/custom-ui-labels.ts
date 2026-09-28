import type { CustomUiComponent } from './custom-ui-api'

type Translate = (key: string) => string

export function customUiComponentLabel(component: CustomUiComponent, t: Translate): string {
  return component.builtIn && component.id === 'pisper-island'
    ? t('custom-ui:builtIn.islandName')
    : component.name
}

export function customUiComponentDescription(component: CustomUiComponent, t: Translate): string {
  return component.builtIn && component.id === 'pisper-island'
    ? t('custom-ui:builtIn.islandDescription')
    : component.description
}
