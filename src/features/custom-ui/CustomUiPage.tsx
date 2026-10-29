import { PageFrame } from '@/components/app/PageFrame'
// 自定义组件页：加载 dataDir/custom-ui/ 下用户自写的静态 UI 组件，
// 左侧组件列表 + 右侧沙箱 iframe 渲染。组件与应用的交互全部经过
// component-bridge 的 postMessage 代理（权限按 manifest 声明过滤）。
import { useRef, useState } from 'react'
import { Link } from 'react-router-dom'
import {
  Blocks,
  FolderCode,
  FolderUp,
  PanelsTopLeft,
  RefreshCw,
  ShieldCheck,
  Upload,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { AppNotice } from '@/components/ui/app-primitives'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import { ApiError } from '@/lib/http'
import type { Notify } from '@/app/route-context'
import { CustomUiFrame } from './CustomUiFrame'
import { importCustomUiBundle } from './custom-ui-api'
import { useCustomUiComponents } from './useCustomUiComponents'
import { customUiComponentLabel, customUiComponentDescription } from './custom-ui-labels'
import { resolveFloatingWidgetIds, useFloatingWidgetsStore } from './floating-widgets-store'
import type { CustomUiComponent } from './custom-ui-api'

type CustomUiPageProps = {
  embedded?: boolean
  notify: Notify
}

function permissionLabel(permission: string, t: ReturnType<typeof useI18n>['t']) {
  if (permission === 'config.read') return t('custom-ui:customUiPage.permissionConfigRead')
  if (permission === 'sessions.read') return t('custom-ui:customUiPage.permissionSessionsRead')
  if (permission === 'notify') return t('custom-ui:customUiPage.permissionNotify')
  if (permission === 'game-assets.read') return t('custom-ui:customUiPage.permissionGameAssetsRead')
  if (permission === 'game-assets.write')
    return t('custom-ui:customUiPage.permissionGameAssetsWrite')
  if (permission === 'game-assets.run') return t('custom-ui:customUiPage.permissionGameAssetsRun')
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
    <section className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-stroke bg-panel">
      {/* 头部：名称 15/500 + 内置徽标 + 等宽版本号，操作在右；说明与权限在下方细线区内。 */}
      <header className="flex min-w-0 flex-wrap items-center gap-2 border-b border-stroke-soft px-3.5 py-2.5">
        <strong className="min-w-0 flex-1 truncate text-[15px] font-medium">
          {customUiComponentLabel(component, t)}
        </strong>
        {component.builtIn && <Badge>{t('custom-ui:builtIn.badge')}</Badge>}
        {component.version && (
          <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
            v{component.version}
          </span>
        )}
        <span className="flex flex-wrap items-center gap-1.5">
          <Button variant="secondary" size="sm" className="max-[650px]:h-11" asChild>
            <Link to={`/tools/components/${encodeURIComponent(component.id)}`}>
              {t('custom-ui:customUiPage.openComponent')}
            </Link>
          </Button>
          <Button
            variant="ghost"
            size="sm"
            aria-pressed={floating}
            title={t('custom-ui:floating.hint')}
            className="aria-pressed:bg-star-soft aria-pressed:text-star-strong max-[650px]:h-11"
            onClick={onToggleFloating}
          >
            <PanelsTopLeft />
            {floating ? t('custom-ui:floating.hide') : t('custom-ui:floating.show')}
          </Button>
        </span>
      </header>
      <div className="flex min-h-0 flex-1 flex-col gap-2.5 p-3.5">
        <p className="text-xs leading-relaxed text-content-muted">{t('custom-ui:floating.hint')}</p>
        {component.description && (
          <p className="text-sm leading-relaxed text-content-secondary">
            {customUiComponentDescription(component, t)}
          </p>
        )}
        {component.permissions.length > 0 && (
          <div className="flex flex-wrap items-center gap-1.5 text-xs text-content-muted">
            <ShieldCheck className="size-3.5 flex-none" aria-hidden="true" />
            {component.permissions.map((permission) => (
              <Badge key={permission}>{permissionLabel(permission, t)}</Badge>
            ))}
          </div>
        )}
        {/* 组件舞台是一口凹陷的井：代码井底色 + 细线，与外层纸面拉开层次。 */}
        <div className="min-h-[240px] flex-1 overflow-hidden rounded-md border border-stroke-soft bg-code">
          <CustomUiFrame component={component} notify={notify} />
        </div>
      </div>
    </section>
  )
}

export function CustomUiPage({ notify, embedded = false }: CustomUiPageProps) {
  const { t } = useI18n()
  const catalog = useCustomUiComponents()
  const components = catalog.data?.components || []
  const root = catalog.data?.root || ''
  const [selectedId, setSelectedId] = useState('')
  const [importing, setImporting] = useState(false)
  const [importError, setImportError] = useState('')
  const [importedName, setImportedName] = useState('')
  const zipInputRef = useRef<HTMLInputElement>(null)
  const folderInputRef = useRef<HTMLInputElement>(null)
  const loading = catalog.isPending
  const refreshing = catalog.isFetching
  const error = catalog.error ? t('custom-ui:widget.catalogFailed') : ''
  const selected = components.find((item) => item.id === selectedId) || components[0] || null
  const prefs = useFloatingWidgetsStore((state) => state.prefs)
  const storageError = useFloatingWidgetsStore((state) => state.storageError)
  const floatingIds = resolveFloatingWidgetIds([], prefs)
  const toggleFloating = () => {
    if (!selected) return
    try {
      useFloatingWidgetsStore.getState().setVisible(selected.id, !floatingIds.includes(selected.id))
    } catch {
      notify(t('custom-ui:floating.storageFailed'))
    }
  }

  const install = async (bundle: Blob) => {
    setImporting(true)
    setImportError('')
    try {
      const result = await importCustomUiBundle(bundle)
      await catalog.refetch()
      setSelectedId(result.id)
      setImportedName(result.name)
    } catch (caught) {
      const code = caught instanceof ApiError ? caught.data?.code : undefined
      setImportError(
        code === 'component_already_installed'
          ? t('custom-ui:customUiPage.importExists')
          : t('custom-ui:customUiPage.importFailed'),
      )
    } finally {
      setImporting(false)
    }
  }

  const importFolder = async (files: File[]) => {
    setImporting(true)
    setImportError('')
    try {
      const selected = files.filter((file) => !file.name.startsWith('.'))
      if (!selected.length || selected.reduce((sum, file) => sum + file.size, 0) > 15 * 1024 * 1024)
        throw new Error('Invalid component folder')
      const entries: Record<string, Uint8Array> = {}
      for (const file of selected) {
        const path = file.webkitRelativePath || file.name
        entries[path] = new Uint8Array(await file.arrayBuffer())
      }
      const { zipSync } = await import('fflate')
      await install(new Blob([zipSync(entries, { level: 0 })], { type: 'application/zip' }))
    } catch {
      setImportError(t('custom-ui:customUiPage.importFailed'))
    } finally {
      setImporting(false)
    }
  }

  const importPanel = (
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <input
        ref={(node) => {
          folderInputRef.current = node
          node?.setAttribute('webkitdirectory', '')
        }}
        type="file"
        multiple
        className="sr-only"
        aria-hidden="true"
        tabIndex={-1}
        onChange={(event) => {
          const files = [...(event.currentTarget.files || [])]
          event.currentTarget.value = ''
          if (files.length) void importFolder(files)
        }}
      />
      <input
        ref={zipInputRef}
        type="file"
        accept=".zip,application/zip"
        className="sr-only"
        aria-hidden="true"
        tabIndex={-1}
        onChange={(event) => {
          const file = event.currentTarget.files?.[0]
          if (file) void install(file)
          event.currentTarget.value = ''
        }}
      />
      <Button
        variant="secondary"
        size="sm"
        className="max-[650px]:h-11"
        disabled={importing}
        onClick={() => folderInputRef.current?.click()}
      >
        <FolderUp />
        {t('custom-ui:customUiPage.importFolder')}
      </Button>
      <Button
        variant="secondary"
        size="sm"
        className="max-[650px]:h-11"
        disabled={importing}
        onClick={() => zipInputRef.current?.click()}
      >
        <Upload />
        {t('custom-ui:customUiPage.importZip')}
      </Button>
      {importing && (
        <span role="status" className="inline-flex items-center gap-1.5 text-xs text-content-muted">
          <OrbitSpinner className="size-3.5" />
          {t('custom-ui:customUiPage.importing')}
        </span>
      )}
      {importError && (
        <span role="alert" className="text-xs text-danger">
          {importError}
        </span>
      )}
    </div>
  )

  const importNotice = (
    <Dialog open={Boolean(importedName)} onOpenChange={(open) => !open && setImportedName('')}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('custom-ui:customUiPage.imported', { name: importedName })}</DialogTitle>
          <DialogDescription>{t('custom-ui:customUiPage.restartHint')}</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={() => setImportedName('')}>
            {t('custom-ui:customUiPage.restartLater')}
          </Button>
          <Button onClick={() => window.location.reload()}>
            {t('custom-ui:customUiPage.reloadNow')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )

  if (loading) return <PageState kind="loading" title={t('custom-ui:customUiPage.loading')} />

  if (!components.length && !error) {
    return (
      <div className="flex min-h-[100%] min-w-0 flex-col gap-[12px]">
        {importPanel}
        {importNotice}
        {/* 空状态：星轨插画 + 说明 + 等宽目录路径，重新扫描是唯一的操作。 */}
        <InlineEmptyState
          className="flex-1"
          title={t('custom-ui:customUiPage.emptyTitle')}
          description={
            <>
              {t('custom-ui:customUiPage.emptyHint')}
              {root && (
                <code className="mx-auto mt-2 block max-w-full rounded-xs bg-inline-code px-2 py-1 font-mono text-xs break-all text-content-secondary">
                  {root}
                </code>
              )}
              <span className="mt-2 block">
                {t('custom-ui:customUiPage.directoryLocationHint')}
              </span>
            </>
          }
          action={
            <Button
              variant="secondary"
              size="sm"
              className="max-[650px]:h-11"
              disabled={refreshing}
              onClick={() => void catalog.refetch()}
            >
              <RefreshCw className={refreshing ? 'animate-spin' : ''} />
              {t('custom-ui:customUiPage.rescan')}
            </Button>
          }
        />
      </div>
    )
  }

  return (
    <PageFrame variant="collection" embedded={embedded}>
      <div className="flex min-w-0 flex-col gap-3">
        {importPanel}
        {importNotice}
        {error && (
          <AppNotice>
            <FolderCode size={14} />
            <span>{error}</span>
          </AppNotice>
        )}
        {storageError && <AppNotice>{t('custom-ui:floating.storageFailed')}</AppNotice>}
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-[minmax(220px,280px)_minmax(0,1fr)]">
          {/* 组件清单：细线卡片，选中行为次级表面 + 3px 强调色刻度；组件 id 用等宽字。 */}
          <section className="flex min-h-0 flex-col overflow-hidden rounded-md border border-stroke bg-panel">
            <header className="flex min-h-10 items-center justify-between gap-2 border-b border-stroke-soft bg-surface-subtle py-1 pr-1.5 pl-3.5">
              <span className="text-xs font-medium text-content-secondary tabular-nums">
                {t('custom-ui:customUiPage.installedComponents', { count: components.length })}
              </span>
              <Button
                variant="ghost"
                size="icon-sm"
                className="max-[650px]:size-11"
                title={t('custom-ui:customUiPage.rescan')}
                aria-label={t('custom-ui:customUiPage.rescan')}
                disabled={refreshing}
                onClick={() => void catalog.refetch()}
              >
                <RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} />
              </Button>
            </header>
            <ul className="flex min-h-0 flex-col gap-0.5 overflow-y-auto p-1.5">
              {components.map((component) => {
                const active = component.id === selected?.id
                return (
                  <li key={component.id} className="relative">
                    {active && (
                      <i
                        aria-hidden="true"
                        className="absolute inset-y-2 left-0 w-[3px] rounded-r-[3px] bg-star"
                      />
                    )}
                    <button
                      type="button"
                      aria-current={active || undefined}
                      onClick={() => setSelectedId(component.id)}
                      className={cn(
                        'flex w-full min-w-0 flex-col gap-0.5 rounded-sm px-2.5 py-2 text-left transition-colors duration-(--d-fast) focus-visible:-outline-offset-2 max-[650px]:min-h-11',
                        active
                          ? 'bg-surface-muted text-content'
                          : 'text-content-secondary hover:bg-surface-subtle',
                      )}
                    >
                      <span className="flex min-w-0 items-center gap-2">
                        <Blocks
                          className="size-3.5 flex-none text-content-muted"
                          aria-hidden="true"
                        />
                        <strong className="min-w-0 flex-1 truncate text-sm font-medium">
                          {customUiComponentLabel(component, t)}
                        </strong>
                        {component.builtIn && <Badge>{t('custom-ui:builtIn.badge')}</Badge>}
                      </span>
                      <small className="truncate pl-5.5 font-mono text-xs text-content-muted">
                        {component.id}
                      </small>
                    </button>
                  </li>
                )
              })}
            </ul>
            <p className="mt-auto border-t border-stroke-soft px-3.5 py-2.5 text-xs leading-relaxed break-all text-content-muted">
              {t('custom-ui:customUiPage.directoryHint', { path: root })}
            </p>
          </section>
          {selected ? (
            <ComponentStage
              key={`${selected.id}:${catalog.dataUpdatedAt}`}
              component={selected}
              notify={notify}
              floating={floatingIds.includes(selected.id)}
              onToggleFloating={toggleFloating}
            />
          ) : (
            <InlineEmptyState title={t('custom-ui:customUiPage.selectComponent')} />
          )}
        </div>
      </div>
    </PageFrame>
  )
}
