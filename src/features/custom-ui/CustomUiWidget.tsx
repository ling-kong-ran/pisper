import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { CustomUiFrame } from './CustomUiFrame'
import { useCustomUiComponents } from './useCustomUiComponents'

export function CustomUiWidget({
  componentId,
  notify,
  preview = false,
}: {
  componentId: string
  notify?: (message: string) => void
  preview?: boolean
}) {
  const { t } = useI18n()
  const catalog = useCustomUiComponents()
  const component = catalog.data?.components.find((item) => item.id === componentId)

  if (component) {
    return (
      <CustomUiFrame key={component.id} component={component} notify={notify} preview={preview} />
    )
  }
  return (
    <div
      role="status"
      className="flex h-full min-h-0 min-w-0 flex-col items-center justify-center gap-2 overflow-auto p-3 text-center text-xs text-[var(--text-muted)]"
    >
      <p className="max-w-full break-words">
        {catalog.isPending
          ? t('custom-ui:customUiPage.loading')
          : catalog.error
            ? t('custom-ui:widget.catalogFailed')
            : t('custom-ui:widget.missing', { id: componentId })}
      </p>
      {!catalog.isPending && (
        <Button
          variant="outline"
          size="sm"
          disabled={catalog.isFetching}
          onClick={() => void catalog.refetch()}
        >
          {t('custom-ui:widget.retry')}
        </Button>
      )}
    </div>
  )
}
