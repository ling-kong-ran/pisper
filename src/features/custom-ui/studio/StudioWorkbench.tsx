// 工作室中栏 · 工作台（规格 5.1）：页头（名称 · 版本 · “已热重载 · N 秒前”，中性灰）与“预览 / 源码 / 日志”分段；
// 页头下是提示词卡片（代理生成 / 修改，坏目录没有 manifest、代理无从改起，不显示），三个分段共用，草稿不随切换丢失；
// 预览页是预览栏 + 点阵舞台 + 状态栏。
import { useEffect, useState, type ReactNode } from 'react'
import { Settings2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import { subscribeComponentChanges } from '@/features/board/component-changes'
import type { ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { LogView } from './LogView'
import type { CommitStore } from './commit-store'
import { PreviewBar, PreviewStage, StudioStatusBar } from './PreviewStage'
import { useComponentErrors } from './use-component-errors'
import { ErrorList } from './StudioSettings'
import { PromptCard } from './PromptCard'
import { SourceView } from './SourceView'
import { componentDisplayName, type PreviewTarget, type PreviewWindow } from './studio-model'

export type WorkbenchTab = 'preview' | 'source' | 'logs'

/** 这个组件最近一次热重载（文件变化带来新 rev）的时刻；只在工作室打开期间记录。 */
function useHotReloadedAt(componentId: string) {
  const [at, setAt] = useState<number | null>(null)
  useEffect(() => {
    setAt(null)
    return subscribeComponentChanges((change) => {
      if (change.id === componentId && change.reason === 'files') setAt(Date.now())
    })
  }, [componentId])
  return at
}

// “已热重载 · N 秒前”：自己计时，只重渲染这一小段。
function HotReloadedLabel({ at }: { at: number }) {
  const { t } = useI18n()
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [])
  const seconds = Math.max(0, Math.round((now - at) / 1000))
  return (
    <span data-studio-reloaded className="text-xs text-content-muted">
      {seconds < 60
        ? t('custom-ui:studio.reloadedSeconds', { count: seconds })
        : t('custom-ui:studio.reloadedMinutes', { count: Math.floor(seconds / 60) })}
    </span>
  )
}

export function StudioWorkbench({
  row,
  tab,
  target,
  window,
  instanceId,
  draft,
  notify,
  commits,
  settingsButton,
  onTab,
  onTarget,
  onWindow,
}: {
  row: ComponentRow
  tab: WorkbenchTab
  target: PreviewTarget
  window: PreviewWindow
  instanceId: string | null
  draft: { title: string; props: Record<string, unknown> }
  notify: Notify
  commits: CommitStore
  // 设置栏收进抽屉时，页头放一个打开它的按钮。
  settingsButton?: ReactNode
  onTab: (tab: WorkbenchTab) => void
  onTarget: (target: PreviewTarget) => void
  onWindow: (window: PreviewWindow) => void
}) {
  const { t, language } = useI18n()
  const runnable =
    row.kind === 'native' && row.status === 'ready' && Boolean(row.manifest && row.rev)
  const reloadedAt = useHotReloadedAt(row.id)
  const errors = useComponentErrors(row.id, row.kind === 'native')
  return (
    <section
      data-studio-workbench
      data-component-stage={row.id}
      data-component-kind={row.kind}
      aria-label={t('custom-ui:studio.workbench')}
      className="flex min-h-0 min-w-0 flex-col"
    >
      <header className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-stroke px-5 py-3">
        <h2 className="flex min-w-0 items-baseline gap-2">
          <span className="truncate text-base font-semibold">
            {componentDisplayName(row, language)}
          </span>
          {row.version && (
            <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
              v{row.version}
            </span>
          )}
        </h2>
        {reloadedAt !== null && <HotReloadedLabel at={reloadedAt} />}
        <span className="ml-auto flex items-center gap-2">
          <SegmentedControl<WorkbenchTab>
            size="sm"
            aria-label={t('custom-ui:studio.workbenchView')}
            value={tab}
            onValueChange={onTab}
            options={[
              { value: 'preview', label: t('custom-ui:studio.tabPreview') },
              { value: 'source', label: t('custom-ui:studio.tabSource') },
              { value: 'logs', label: t('custom-ui:studio.tabLogs') },
            ]}
          />
          {settingsButton}
        </span>
      </header>
      <div className={cn('flex min-h-0 flex-1 flex-col gap-3 px-5 pt-4 pb-3')}>
        {row.kind === 'native' && <PromptCard row={row} notify={notify} className="flex-none" />}
        {tab === 'source' ? (
          <SourceView row={row} />
        ) : tab === 'logs' ? (
          <LogView componentId={row.id} errors={errors.data} errorsFailed={Boolean(errors.error)} />
        ) : runnable ? (
          <>
            <PreviewBar
              manifest={row.manifest}
              target={target}
              window={window}
              onTarget={onTarget}
              onWindow={onWindow}
            />
            <PreviewStage
              row={row}
              target={target}
              window={window}
              instanceId={instanceId}
              title={draft.title}
              props={draft.props}
              notify={notify}
              commits={commits}
            />
            <StudioStatusBar componentId={row.id} errors={errors.data} commits={commits} />
          </>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
            <ErrorList row={row} />
            <InlineEmptyState
              className="flex-1"
              title={
                row.kind === 'broken'
                  ? t('custom-ui:studio.brokenTitle')
                  : t('custom-ui:studio.invalidTitle')
              }
              description={
                row.kind === 'broken'
                  ? t('custom-ui:studio.brokenHint')
                  : t('custom-ui:studio.invalidHint')
              }
            />
          </div>
        )}
      </div>
    </section>
  )
}

export function SettingsDrawerButton({ onOpen }: { onOpen: () => void }) {
  const { t } = useI18n()
  return (
    <Button
      size="sm"
      variant="secondary"
      data-studio-settings-open
      aria-label={t('custom-ui:studio.openSettings')}
      onClick={onOpen}
    >
      <Settings2 />
      {t('custom-ui:studio.settings')}
    </Button>
  )
}
