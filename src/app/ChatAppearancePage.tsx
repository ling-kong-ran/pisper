// 界面设置：外观设置之上放一条“组件已移到 组件 页 →”的跳转（规格 2.6）。独立组件的导入、预览与
// 悬浮显示都在“组件”标签里管理，设置里不再有“独立组件”标签；旧链接 ?view=widgets（含更早的
// ?view=layout）直接反向重定向到组件页。文案在随设置路由加载的 custom-ui 命名空间里，不占入口包。
import type { ReactNode } from 'react'
import { Link, Navigate, useSearchParams } from 'react-router-dom'
import { PAGE_PATHS } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'

export function ChatAppearancePage({ appearance }: { appearance: ReactNode }) {
  const { t } = useI18n()
  const [params] = useSearchParams()
  const view = params.get('view')
  if (view === 'layout' || view === 'widgets')
    return <Navigate to={PAGE_PATHS.components} replace />
  return (
    <div className="flex min-h-full min-w-0 flex-col gap-4">
      <Link
        to={PAGE_PATHS.components}
        data-components-moved
        className="group flex min-h-11 w-fit max-w-full items-center gap-x-2 gap-y-0.5 rounded-md border border-stroke bg-solid px-3 py-2 text-sm text-content-secondary transition-colors duration-(--d-fast) hover:border-stroke-strong hover:text-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:w-full max-[650px]:flex-wrap"
      >
        <span className="font-medium text-content">{t('custom-ui:moved.title')}</span>
        <span className="text-xs text-content-muted">{t('custom-ui:moved.hint')}</span>
      </Link>
      {appearance}
    </div>
  )
}
