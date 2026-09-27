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
    <div className="flex min-w-0 flex-col gap-3">
      <div className="flex items-center justify-between gap-3">
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
      </div>
      <div className="h-[max(520px,calc(100dvh-180px))] min-w-0 overflow-hidden rounded-xl border bg-card">
        {component ? (
          <CustomUiFrame component={component} notify={notify} />
        ) : (
          <div
            className="flex h-full flex-col items-center justify-center gap-3 p-4 text-sm text-muted-foreground"
            role="status"
          >
            {query.isPending
              ? t('custom-ui:customUiPage.loading')
              : t('custom-ui:widget.catalogFailed')}
            {!query.isPending && (
              <Button variant="outline" onClick={() => void query.refetch()}>
                {t('custom-ui:widget.retry')}
              </Button>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
