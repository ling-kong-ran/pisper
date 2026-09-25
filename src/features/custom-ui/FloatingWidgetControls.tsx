import { useId } from 'react'
import { RefreshCw } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Label } from '@/components/ui/label'
import { customUiComponentLabel } from './custom-ui-labels'
import { resolveFloatingWidgetIds, useFloatingWidgetsStore } from './floating-widgets-store'
import { useCustomUiComponents } from './useCustomUiComponents'

export function FloatingWidgetControls({
  defaults,
  notify,
}: {
  defaults: readonly string[]
  notify: (message: string) => void
}) {
  const { t } = useI18n()
  const labelId = useId()
  const catalog = useCustomUiComponents()
  const prefs = useFloatingWidgetsStore((state) => state.prefs)
  const visibleIds = resolveFloatingWidgetIds(defaults, prefs)
  const components = catalog.data?.components ?? []
  const setVisible = (id: string, checked: boolean) => {
    try {
      useFloatingWidgetsStore.getState().setVisible(id, checked)
      if (useFloatingWidgetsStore.getState().storageError)
        notify(t('custom-ui:floating.storageFailed'))
    } catch {
      notify(t('custom-ui:floating.storageFailed'))
    }
  }
  return (
    <section className="min-w-0 space-y-2" aria-labelledby={labelId}>
      <div className="flex items-center justify-between gap-2">
        <h3 id={labelId} className="text-xs font-medium text-muted-foreground">
          {t('custom-ui:floating.title')}
        </h3>
        <Button
          variant="ghost"
          size="icon-sm"
          disabled={catalog.isFetching}
          aria-label={t('custom-ui:customUiPage.rescan')}
          title={t('custom-ui:customUiPage.rescan')}
          onClick={() => void catalog.refetch()}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </div>
      {catalog.isPending && (
        <p role="status" className="text-xs text-muted-foreground">
          {t('custom-ui:customUiPage.loading')}
        </p>
      )}
      {catalog.error && (
        <p role="status" className="text-xs text-muted-foreground">
          {t('custom-ui:widget.catalogFailed')}
        </p>
      )}
      {!catalog.isPending && !catalog.error && !components.length && (
        <p className="text-xs text-muted-foreground">{t('custom-ui:customUiPage.emptyTitle')}</p>
      )}
      {components.length > 0 && (
        <div className="max-h-40 space-y-0.5 overflow-y-auto">
          {components.map((component) => {
            const id = `${labelId}-${component.id}`
            return (
              <div key={component.id} className="flex min-h-10 min-w-0 items-center gap-3 px-1">
                <Checkbox
                  id={id}
                  checked={visibleIds.includes(component.id)}
                  onCheckedChange={(checked) => setVisible(component.id, checked === true)}
                />
                <Label htmlFor={id} className="min-w-0 flex-1 cursor-pointer py-2 font-normal">
                  <span className="truncate">{customUiComponentLabel(component, t)}</span>
                </Label>
              </div>
            )
          })}
        </div>
      )}
    </section>
  )
}
