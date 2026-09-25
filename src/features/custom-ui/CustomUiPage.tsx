// 自定义组件页：加载 dataDir/custom-ui/ 下用户自写的静态 UI 组件，
// 左侧组件列表 + 右侧沙箱 iframe 渲染。组件与应用的交互全部经过
// component-bridge 的 postMessage 代理（权限按 manifest 声明过滤）。
import { useState } from 'react'
import { Blocks, FolderCode, PanelsTopLeft, RefreshCw, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppCard as Panel, AppEmptyState, AppNotice } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import { CustomUiFrame } from './CustomUiFrame'
import { useCustomUiComponents } from './useCustomUiComponents'
import { customUiComponentLabel, customUiComponentDescription } from './custom-ui-labels'
import { resolveFloatingWidgetIds, useFloatingWidgetsStore } from './floating-widgets-store'
import type { CustomUiComponent } from './custom-ui-api'

type CustomUiPageProps = {
  notify: Notify
  floatingDefaults?: readonly string[]
}

function permissionLabel(permission: string, t: ReturnType<typeof useI18n>['t']) {
  if (permission === 'config.read') return t('custom-ui:customUiPage.permissionConfigRead')
  if (permission === 'sessions.read') return t('custom-ui:customUiPage.permissionSessionsRead')
  if (permission === 'notify') return t('custom-ui:customUiPage.permissionNotify')
  return permission
}

// 单个组件的渲染面板：iframe 以 opaque origin 沙箱加载，挂载后接桥。
function ComponentStage({
  component,
  notify,
  floating,
  onToggleFloating,
}: {
  component: CustomUiComponent
  notify: Notify
  floating: boolean
  onToggleFloating: () => void
}) {
  const { t } = useI18n()

  return (
    <Panel className="flex min-h-0 min-w-0 flex-1 flex-col gap-2 overflow-hidden p-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[14px]">
          {customUiComponentLabel(component, t)}
        </strong>
        {component.builtIn && (
          <span className="rounded bg-[var(--surface-muted)] px-1.5 py-0.5 text-xs text-[var(--text-muted)]">
            {t('custom-ui:builtIn.badge')}
          </span>
        )}
        {component.version && (
          <span className="flex-none text-[12px] text-[var(--text-muted)]">
            v{component.version}
          </span>
        )}
        <Button
          variant="ghost"
          size="sm"
          aria-pressed={floating}
          title={t('custom-ui:floating.hint')}
          onClick={onToggleFloating}
        >
          <PanelsTopLeft size={13} />
          {floating ? t('custom-ui:floating.hide') : t('custom-ui:floating.show')}
        </Button>
      </div>
      <p className="m-0 text-xs leading-5 text-[var(--text-muted)]">
        {t('custom-ui:floating.hint')}
      </p>
      {component.description && (
        <p className="m-0 text-[12px] leading-[1.55] text-[var(--text-secondary)]">
          {customUiComponentDescription(component, t)}
        </p>
      )}
      {component.permissions.length > 0 && (
        <div className="flex flex-wrap items-center gap-[6px] text-[11px] text-[var(--text-muted)]">
          <ShieldCheck size={12} className="flex-none" />
          {component.permissions.map((permission) => (
            <span
              key={permission}
              className="rounded-[var(--r-sm)] bg-[var(--surface-muted)] px-[6px] py-[2px]"
            >
              {permissionLabel(permission, t)}
            </span>
          ))}
        </div>
      )}
      <div className="min-h-[240px] flex-1 overflow-hidden rounded-[var(--r-md)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)]">
        <CustomUiFrame component={component} notify={notify} />
      </div>
    </Panel>
  )
}

export function CustomUiPage({ notify, floatingDefaults = [] }: CustomUiPageProps) {
  const { t } = useI18n()
  const catalog = useCustomUiComponents()
  const components = catalog.data?.components || []
  const root = catalog.data?.root || ''
  const [selectedId, setSelectedId] = useState('')
  const loading = catalog.isPending
  const refreshing = catalog.isFetching
  const error = catalog.error ? t('custom-ui:widget.catalogFailed') : ''
  const selected = components.find((item) => item.id === selectedId) || components[0] || null
  const prefs = useFloatingWidgetsStore((state) => state.prefs)
  const storageError = useFloatingWidgetsStore((state) => state.storageError)
  const floatingIds = resolveFloatingWidgetIds(floatingDefaults, prefs)
  const toggleFloating = () => {
    if (!selected) return
    try {
      useFloatingWidgetsStore.getState().setVisible(selected.id, !floatingIds.includes(selected.id))
    } catch {
      notify(t('custom-ui:floating.storageFailed'))
    }
  }

  if (loading) {
    return (
      <div className="flex min-h-[100%] items-center justify-center text-[12px] text-[var(--text-muted)]">
        {t('custom-ui:customUiPage.loading')}
      </div>
    )
  }

  if (!components.length && !error) {
    return (
      <div className="flex min-h-[100%] min-w-0 flex-col gap-[12px]">
        <AppEmptyState className="flex flex-1 flex-col items-center justify-center gap-[10px] p-[40px_24px] text-center">
          <Blocks size={28} className="text-[var(--text-muted)]" />
          <strong className="text-[14px]">{t('custom-ui:customUiPage.emptyTitle')}</strong>
          <p className="m-0 max-w-[520px] text-[12px] leading-[1.7] text-[var(--text-secondary)]">
            {t('custom-ui:customUiPage.emptyHint')}
          </p>
          {root && (
            <code className="max-w-full break-all rounded-[var(--r-sm)] bg-[var(--surface-muted)] px-[8px] py-[4px] text-[11px] text-[var(--text-soft)]">
              {root}
            </code>
          )}
          <p className="m-0 max-w-[520px] text-[11px] leading-[1.6] text-[var(--text-muted)]">
            {t('custom-ui:customUiPage.directoryLocationHint')}
          </p>
          <Button
            variant="outline"
            size="sm"
            disabled={refreshing}
            onClick={() => void catalog.refetch()}
          >
            <RefreshCw size={13} className={refreshing ? 'animate-spin' : ''} />
            {t('custom-ui:customUiPage.rescan')}
          </Button>
        </AppEmptyState>
      </div>
    )
  }

  return (
    <div className="flex min-h-[100%] min-w-0 flex-col gap-[12px]">
      {error && (
        <AppNotice>
          <FolderCode size={14} />
          <span>{error}</span>
        </AppNotice>
      )}
      {storageError && <AppNotice>{t('custom-ui:floating.storageFailed')}</AppNotice>}
      <div className="grid min-h-0 flex-1 grid-cols-1 gap-[12px] lg:grid-cols-[minmax(220px,280px)_minmax(0,1fr)]">
        <Panel className="flex min-h-0 flex-col gap-[8px] overflow-y-auto p-2">
          <div className="flex items-center justify-between px-1 pt-1">
            <span className="text-[12px] font-semibold text-[var(--text-secondary)]">
              {t('custom-ui:customUiPage.installedComponents', { count: components.length })}
            </span>
            <Button
              variant="ghost"
              size="icon"
              className="h-[26px] w-[26px]"
              title={t('custom-ui:customUiPage.rescan')}
              aria-label={t('custom-ui:customUiPage.rescan')}
              disabled={refreshing}
              onClick={() => void catalog.refetch()}
            >
              <RefreshCw size={13} className={refreshing ? 'animate-spin' : ''} />
            </Button>
          </div>
          {components.map((component) => (
            <button
              key={component.id}
              type="button"
              onClick={() => setSelectedId(component.id)}
              className={cn(
                'flex min-w-0 cursor-pointer flex-col gap-[2px] rounded-[var(--r-md)] border-0 px-[10px] py-[8px] text-left transition-colors',
                component.id === selected?.id
                  ? 'bg-[var(--accent-soft)] text-[var(--text)]'
                  : 'bg-transparent text-[var(--text-secondary)] hover:bg-[var(--surface-muted)]',
              )}
            >
              <span className="flex min-w-0 items-center gap-[6px]">
                <Blocks size={13} className="flex-none text-[var(--star-strong)]" />
                <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                  {customUiComponentLabel(component, t)}
                </strong>
              </span>
              {component.builtIn && (
                <small className="pl-[19px] text-xs text-[var(--text-muted)]">
                  {t('custom-ui:builtIn.badge')}
                </small>
              )}
              <small className="overflow-hidden text-ellipsis whitespace-nowrap pl-[19px] text-[11px] text-[var(--text-muted)]">
                {component.id}
              </small>
            </button>
          ))}
          <p className="m-0 mt-auto break-all px-1 pb-1 text-[11px] leading-[1.6] text-[var(--text-muted)]">
            {t('custom-ui:customUiPage.directoryHint', { path: root })}
          </p>
        </Panel>
        {selected ? (
          <ComponentStage
            key={`${selected.id}:${catalog.dataUpdatedAt}`}
            component={selected}
            notify={notify}
            floating={floatingIds.includes(selected.id)}
            onToggleFloating={toggleFloating}
          />
        ) : (
          <Panel className="grid place-items-center text-[12px] text-[var(--text-muted)]">
            {t('custom-ui:customUiPage.selectComponent')}
          </Panel>
        )}
      </div>
    </div>
  )
}
