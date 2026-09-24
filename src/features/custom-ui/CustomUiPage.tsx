// 自定义组件页：加载 dataDir/custom-ui/ 下用户自写的静态 UI 组件，
// 左侧组件列表 + 右侧沙箱 iframe 渲染。组件与应用的交互全部经过
// component-bridge 的 postMessage 代理（权限按 manifest 声明过滤）。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Blocks, FolderCode, RefreshCw, ShieldCheck } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { AppCard as Panel, AppEmptyState, AppNotice } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import { attachComponentBridge } from './component-bridge'
import {
  listCustomUiComponents,
  createCustomUiView,
  renewCustomUiView,
  releaseCustomUiView,
  type CustomUiComponent,
  type CustomUiView,
} from './custom-ui-api'

type CustomUiPageProps = {
  notify: Notify
}

function permissionLabel(permission: string, t: ReturnType<typeof useI18n>['t']) {
  if (permission === 'config.read') return t('custom-ui:customUiPage.permissionConfigRead')
  if (permission === 'sessions.read') return t('custom-ui:customUiPage.permissionSessionsRead')
  if (permission === 'notify') return t('custom-ui:customUiPage.permissionNotify')
  return permission
}

// 单个组件的渲染面板：iframe 以 opaque origin 沙箱加载，挂载后接桥。
function ComponentStage({ component, notify }: { component: CustomUiComponent; notify: Notify }) {
  const { t } = useI18n()
  const frameRef = useRef<HTMLIFrameElement | null>(null)
  const [failed, setFailed] = useState(false)
  const [view, setView] = useState<CustomUiView | null>(null)

  useEffect(() => {
    const controller = new AbortController()
    let activeView: CustomUiView | null = null
    let timer: ReturnType<typeof setTimeout> | undefined
    setFailed(false)
    setView(null)
    // 父页面持有鉴权，定期续期；卸载主动撤销，异常断开时服务端五分钟自动过期。
    const renew = async () => {
      if (!activeView || controller.signal.aborted) return
      try {
        await renewCustomUiView(activeView.id, controller.signal)
        if (!controller.signal.aborted) timer = setTimeout(() => void renew(), 60_000)
      } catch {
        if (!controller.signal.aborted) setFailed(true)
      }
    }
    void createCustomUiView(component.id, controller.signal)
      .then((next) => {
        activeView = next
        if (controller.signal.aborted) {
          void releaseCustomUiView(next.id).catch(() => undefined)
          return
        }
        setView(next)
        timer = setTimeout(() => void renew(), 60_000)
      })
      .catch(() => {
        if (!controller.signal.aborted) setFailed(true)
      })
    return () => {
      controller.abort()
      clearTimeout(timer)
      // 撤销失败不重试写操作；服务器 TTL 保证失联预览最终被清理。
      if (activeView) void releaseCustomUiView(activeView.id).catch(() => undefined)
    }
  }, [component])

  useEffect(() => {
    const iframe = frameRef.current
    if (!iframe) return
    return attachComponentBridge(iframe, { component, notify: (message) => notify(message) })
  }, [component, notify, view])

  return (
    <Panel className="flex min-h-0 min-w-0 flex-1 flex-col gap-2 overflow-hidden p-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[14px]">
          {component.name}
        </strong>
        {component.version && (
          <span className="flex-none text-[12px] text-[var(--text-muted)]">
            v{component.version}
          </span>
        )}
      </div>
      {component.description && (
        <p className="m-0 text-[12px] leading-[1.55] text-[var(--text-secondary)]">
          {component.description}
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
      {failed ? (
        <div className="grid min-h-[240px] flex-1 place-items-center rounded-[var(--r-md)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)] text-[12px] text-[var(--text-muted)]">
          {t('custom-ui:customUiPage.componentFailedToLoad')}
        </div>
      ) : !view ? (
        <p>{t('custom-ui:customUiPage.loading')}</p>
      ) : (
        <iframe
          key={component.id}
          ref={frameRef}
          title={component.name}
          src={view.entryUrl}
          referrerPolicy="no-referrer"
          // 不加 allow-same-origin：组件处于 opaque origin，只能经桥与应用交互。
          sandbox="allow-scripts"
          className="min-h-[240px] flex-1 rounded-[var(--r-md)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)]"
          onError={() => setFailed(true)}
        />
      )}
    </Panel>
  )
}

export function CustomUiPage({ notify }: CustomUiPageProps) {
  const { t } = useI18n()
  const [components, setComponents] = useState<CustomUiComponent[]>([])
  const [root, setRoot] = useState('')
  const [selectedId, setSelectedId] = useState('')
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState('')

  const load = useCallback(async (initial: boolean) => {
    if (initial) setLoading(true)
    else setRefreshing(true)
    setError('')
    try {
      const data = await listCustomUiComponents()
      setComponents(data.components)
      setRoot(data.root)
      setSelectedId((current) =>
        data.components.some((item) => item.id === current)
          ? current
          : data.components[0]?.id || '',
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
      setRefreshing(false)
    }
  }, [])

  useEffect(() => {
    void load(true)
  }, [load])

  const selected = components.find((item) => item.id === selectedId) || null

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
            onClick={() => void load(false)}
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
              onClick={() => void load(false)}
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
                component.id === selectedId
                  ? 'bg-[var(--accent-soft)] text-[var(--text)]'
                  : 'bg-transparent text-[var(--text-secondary)] hover:bg-[var(--surface-muted)]',
              )}
            >
              <span className="flex min-w-0 items-center gap-[6px]">
                <Blocks size={13} className="flex-none text-[var(--star-strong)]" />
                <strong className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-[13px]">
                  {component.name}
                </strong>
              </span>
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
          <ComponentStage key={selected.id} component={selected} notify={notify} />
        ) : (
          <Panel className="grid place-items-center text-[12px] text-[var(--text-muted)]">
            {t('custom-ui:customUiPage.selectComponent')}
          </Panel>
        )}
      </div>
    </div>
  )
}
