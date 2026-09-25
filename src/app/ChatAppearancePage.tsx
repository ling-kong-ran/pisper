// 界面设置由应用层组装三个公开功能；配置域通过插槽交出原有外观设置。
import { lazy, Suspense, type ReactNode } from 'react'
import { useSearchParams } from 'react-router-dom'
import { useI18n } from '@/app/use-i18n'
import { ensureChatLayoutMessages, ensureCustomUiMessages } from '@/app/i18n'
import type { Notify } from '@/app/route-context'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useFloatingWidgetDefaults } from '@/app/useFloatingWidgetDefaults'

const ChatLayoutEditor = lazy(async () => {
  const [{ ChatLayoutEditor }] = await Promise.all([
    import('@/features/chat/layout/editor'),
    ensureChatLayoutMessages(),
    ensureCustomUiMessages(),
  ])
  return { default: ChatLayoutEditor }
})
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
  const floatingDefaults = useFloatingWidgetDefaults()
  const [params, setParams] = useSearchParams()
  const view = params.get('view')
  const tab = view === 'layout' || view === 'widgets' ? view : 'appearance'
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
      <TabsList aria-label={t('config:interfaceSettings.tabsLabel')} className="max-w-full">
        <TabsTrigger value="appearance">{t('config:interfaceSettings.appearanceTab')}</TabsTrigger>
        <TabsTrigger value="layout">{t('config:interfaceSettings.chatLayoutTab')}</TabsTrigger>
        <TabsTrigger value="widgets">{t('config:interfaceSettings.customUiTab')}</TabsTrigger>
      </TabsList>
      <TabsContent value="appearance" className="min-w-0">
        {appearance}
      </TabsContent>
      <TabsContent value="layout" className="min-w-0" data-config-card="interface-chat-layout">
        <Suspense fallback={<p role="status">{t('common:webPreview.loading')}</p>}>
          <ChatLayoutEditor />
        </Suspense>
      </TabsContent>
      <TabsContent
        value="widgets"
        className="min-h-0 min-w-0 flex-1"
        data-config-card="interface-custom-ui"
      >
        <Suspense fallback={<p role="status">{t('common:webPreview.loading')}</p>}>
          <CustomUiPage notify={notify} floatingDefaults={floatingDefaults} />
        </Suspense>
      </TabsContent>
    </Tabs>
  )
}
