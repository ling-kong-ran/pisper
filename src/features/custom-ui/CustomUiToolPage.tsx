import { PageFrame } from '@/components/app/PageFrame'
import { PageToolbar } from '@/components/app/PageToolbar'
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
  return (
    <PageFrame variant="workbench">
      <div className="flex h-full min-h-0 min-w-0 flex-col p-3">
        <PageToolbar>
          <Button variant="ghost" size="sm" asChild>
            <Link to="/chat">
              <ArrowLeft />
              {t('custom-ui:tool.back')}
            </Link>
          </Button>
          <Button variant="ghost" size="sm" asChild>
            <Link to="/config/interface?view=widgets">
              <Settings2 />
              {t('custom-ui:tool.openSettings')}
            </Link>
          </Button>
        </PageToolbar>
        <div className="min-h-0 min-w-0 flex-1 overflow-hidden rounded-xl border bg-card">
          {component ? (
            <CustomUiFrame component={component} notify={notify} />
          ) : (
            <div
              className="flex h-full flex-col items-center justify-center gap-3 p-4 text-sm text-muted-foreground"
              role="status"
            >
              {query.isPending
                ? t('custom-ui:customUiPage.loading')
                : query.error
                  ? t('custom-ui:widget.catalogFailed')
                  : t('custom-ui:widget.missing', { id: componentId })}
              {!query.isPending && (
                <Button variant="outline" onClick={() => void query.refetch()}>
                  {t('custom-ui:widget.retry')}
                </Button>
              )}
            </div>
          )}
        </div>
      </div>
    </PageFrame>
  )
}
