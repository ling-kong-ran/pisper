import { lazy, Suspense } from 'react'
import { useI18n } from '@/app/use-i18n'
import { ensureCustomUiMessages } from '@/app/i18n'

// 只有画布实际包含独立组件时才加载沙箱宿主，普通会话不承担组件运行时的依赖。
const CustomUiWidget = lazy(async () => {
  const [{ CustomUiWidget }] = await Promise.all([
    import('@/features/custom-ui/widget'),
    ensureCustomUiMessages(),
  ])
  return { default: CustomUiWidget }
})

export function CanvasCustomUi({
  componentId,
  preview = false,
  notify,
}: {
  componentId: string
  preview?: boolean
  notify?: (message: string) => void
}) {
  const { t } = useI18n()
  return (
    <Suspense
      fallback={
        <p role="status" className="p-3 text-xs text-muted-foreground">
          {t('common:webPreview.loading')}
        </p>
      }
    >
      <CustomUiWidget componentId={componentId} preview={preview} notify={notify} />
    </Suspense>
  )
}
