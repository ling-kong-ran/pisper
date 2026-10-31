// 功能 › 我的组件的整页宿主（放置 feature-page，规格 5.5）：/tools/components/:componentId 渲染该组件的
// feature-page 实例。没有实例（或组件停用、不可运行）时如实说明，并给出“在工作室打开”。
import { Suspense } from 'react'
import { Link } from 'react-router-dom'
import { ArrowLeft, Settings2 } from 'lucide-react'
import { PageFrame } from '@/components/app/PageFrame'
import { PageState } from '@/components/app/PageState'
import { PAGE_PATHS, componentPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import type { Notify } from '@/app/route-context'
import type { ComponentRow } from './custom-ui-api'
import { LazyComponentHost, useComponentInstances } from './placements'
import { useComponentFix } from './studio/use-component-agent'

// 整页宿主里的组件：错误卡的 [让代理修复] 交给这个组件的绑定会话。
function FeaturePageHost({
  component,
  instance,
  notify,
}: {
  component: ComponentRow
  instance: { instanceId: string; title: string; props: Record<string, unknown> }
  notify: Notify
}) {
  const fix = useComponentFix(component, notify)
  return (
    <LazyComponentHost
      component={component}
      instance={{
        instanceId: instance.instanceId,
        surface: 'feature-page',
        title: instance.title,
        props: instance.props,
      }}
      notify={(message) => notify(message)}
      onRequestFix={fix}
      className="h-full"
    />
  )
}

export function CustomUiToolPage({ componentId, notify }: { componentId: string; notify: Notify }) {
  const { t } = useI18n()
  const result = useComponentInstances('feature-page')
  const placed = result.placed.find((entry) => entry.component.id === componentId)
  const studioLink = componentPath(componentId)
  // 工具页铺满主纸面：44px 工具条以细线与组件舞台分隔，舞台本身不再套第二层卡片。
  return (
    <PageFrame variant="workbench">
      <div className="flex h-full min-h-0 min-w-0 flex-col">
        <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-1 border-b border-stroke-soft px-2 py-1.5">
          <Button variant="ghost" size="sm" className="max-[650px]:h-11" asChild>
            <Link to="/more">
              <ArrowLeft />
              {t('custom-ui:tool.back')}
            </Link>
          </Button>
          {placed && (
            <strong className="min-w-0 flex-1 truncate px-1 text-sm font-medium">
              {placed.instance.title || placed.component.manifest?.name || placed.component.name}
            </strong>
          )}
          <Button variant="ghost" size="sm" className="ml-auto max-[650px]:h-11" asChild>
            {/* 组件管理在组件工作室（应用内链接直接指向组件页，不经旧地址转一跳）。 */}
            <Link to={PAGE_PATHS.components}>
              <Settings2 />
              {t('custom-ui:tool.openSettings')}
            </Link>
          </Button>
        </div>
        <div
          className="min-h-0 min-w-0 flex-1 overflow-hidden"
          data-feature-page-host={componentId}
        >
          {placed ? (
            <Suspense
              fallback={<PageState kind="loading" title={t('custom-ui:customUiPage.loading')} />}
            >
              <FeaturePageHost
                key={placed.instance.instanceId}
                component={placed.component}
                instance={placed.instance}
                notify={notify}
              />
            </Suspense>
          ) : result.isLoading ? (
            <PageState kind="loading" title={t('custom-ui:customUiPage.loading')} />
          ) : result.error ? (
            <PageState
              kind="error"
              title={t('custom-ui:widget.catalogFailed')}
              action={
                <Button variant="secondary" onClick={result.refetch}>
                  {t('custom-ui:widget.retry')}
                </Button>
              }
            />
          ) : (
            <PageState
              kind="empty"
              title={t('custom-ui:tool.notPlacedTitle')}
              description={t('custom-ui:tool.notPlacedHint')}
              action={
                <Button variant="secondary" asChild>
                  <Link to={studioLink}>{t('custom-ui:tool.openInStudio')}</Link>
                </Button>
              }
            />
          )}
        </div>
      </div>
    </PageFrame>
  )
}
