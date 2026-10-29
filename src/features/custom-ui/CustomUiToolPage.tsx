import { PageFrame } from '@/components/app/PageFrame'
import { PageState } from '@/components/app/PageState'
import { Link } from 'react-router-dom'
import { ArrowLeft, Settings2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { useCustomUiComponents } from './useCustomUiComponents'
import { CustomUiFrame } from './CustomUiFrame'
import type { Notify } from '@/app/route-context'

// 工具页只承载组件；游戏素材/未来视频工作台的状态与操作由各领域桥持有。
export function CustomUiToolPage({ componentId, notify }: { componentId: string; notify: Notify }) {
  const { t } = useI18n()
  const query = useCustomUiComponents()
  const component = query.data?.components.find((item) => item.id === componentId)
  // 工具页铺满主纸面：44px 工具条以细线与组件舞台分隔，舞台本身不再套第二层卡片。
  return (
    <PageFrame variant="workbench">
      <div className="flex h-full min-h-0 min-w-0 flex-col">
        <div className="flex min-h-11 shrink-0 flex-wrap items-center gap-1 border-b border-stroke-soft px-2 py-1.5">
          <Button variant="ghost" size="sm" className="max-[650px]:h-11" asChild>
            <Link to="/chat">
              <ArrowLeft />
              {t('custom-ui:tool.back')}
            </Link>
          </Button>
          <Button variant="ghost" size="sm" className="max-[650px]:h-11" asChild>
            <Link to="/config/interface?view=widgets">
              <Settings2 />
              {t('custom-ui:tool.openSettings')}
            </Link>
          </Button>
        </div>
        <div className="min-h-0 min-w-0 flex-1 overflow-hidden">
          {component ? (
            <CustomUiFrame component={component} notify={notify} />
          ) : query.isPending ? (
            <PageState kind="loading" title={t('custom-ui:customUiPage.loading')} />
          ) : (
            <PageState
              kind="error"
              title={
                query.error
                  ? t('custom-ui:widget.catalogFailed')
                  : t('custom-ui:widget.missing', { id: componentId })
              }
              action={
                <Button variant="secondary" onClick={() => void query.refetch()}>
                  {t('custom-ui:widget.retry')}
                </Button>
              }
            />
          )}
        </div>
      </div>
    </PageFrame>
  )
}
