// 界面设置组合外观与独立组件入口；独立组件的管理视图已迁到“组件”标签页。
import { lazy, Suspense, type ReactNode } from 'react'
import { Navigate, useSearchParams } from 'react-router-dom'
import { PAGE_PATHS } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { ensureCustomUiMessages } from '@/app/i18n'
import type { Notify } from '@/app/route-context'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
const CustomUiPage = lazy(async () => {
  const [{ CustomUiPage }] = await Promise.all([
    import('@/features/custom-ui/public'),
    ensureCustomUiMessages(),
  ])
  return { default: CustomUiPage }
})

export function ChatAppearancePage({
  appearance,
  notify,
}: {
  appearance: ReactNode
  notify: Notify
}) {
  const { t } = useI18n()
  const [params, setParams] = useSearchParams()
  const view = params.get('view')
  const tab = view === 'layout' || view === 'widgets' ? 'widgets' : 'appearance'
  // 独立组件已迁到“组件”标签：旧链接 ?view=widgets（含更早的 layout）反向重定向过去。
  // 这里的标签页本身留到第 4 期设置重组时移除，点它同样会落到组件页。
  if (tab === 'widgets') return <Navigate to={PAGE_PATHS.components} replace />
  return (
    <Tabs
      value={tab}
      onValueChange={(value) => {
        setParams(
          (current) => {
            const next = new URLSearchParams(current)
            if (value === 'appearance') next.delete('view')
            else next.set('view', value)
            return next
          },
          { replace: true },
        )
      }}
      className="min-h-full min-w-0 gap-4"
    >
      {/* 触屏与窄窗下分段项本身达到 44px 触控高度，轨道随之撑高。 */}
      <TabsList
        aria-label={t('config:interfaceSettings.tabsLabel')}
        className="max-w-full pointer-coarse:h-auto max-[650px]:h-auto"
      >
        <TabsTrigger
          value="appearance"
          className="px-3 pointer-coarse:min-h-11 max-[650px]:min-h-11"
        >
          {t('config:interfaceSettings.appearanceTab')}
        </TabsTrigger>
        <TabsTrigger value="widgets" className="px-3 pointer-coarse:min-h-11 max-[650px]:min-h-11">
          {t('config:interfaceSettings.customUiTab')}
        </TabsTrigger>
      </TabsList>
      <TabsContent value="appearance" className="min-w-0">
        {appearance}
      </TabsContent>
      <TabsContent
        value="widgets"
        className="min-h-0 min-w-0 flex-1"
        data-config-card="interface-custom-ui"
      >
        <Suspense fallback={<p role="status">{t('common:webPreview.loading')}</p>}>
          <CustomUiPage notify={notify} embedded />
        </Suspense>
      </TabsContent>
    </Tabs>
  )
}
