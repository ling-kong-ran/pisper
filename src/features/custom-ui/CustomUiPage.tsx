import { PageFrame } from '@/components/app/PageFrame'
// 组件页（A7 S4 过渡版，S5 换成三栏组件工作室）：原生组件（v2）与无法读取的目录列在左边，
// 右边是选中组件的预览（ComponentHost，preview 模式）加启用开关与删除（移入回收站）。
// 旧 iframe 组件与专注岛已删除：启动迁移把旧组件移进回收站，迁移后才手放进来的旧格式目录这里不展示（负责人决定）。
// 手机外壳（规格 7.5）：列表 → 详情两级，每级一屏、44px 触控；选中项由路由 /components/:id 承载。
import { Suspense, useState } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { Blocks, ChevronLeft, ChevronRight, FolderCode, RefreshCw, Trash2 } from 'lucide-react'
import {
  FLOATING_DEFAULT_SIZE,
  OVERVIEW_CARD_DIMENSIONS,
  type ComponentSurface,
} from '@shared/custom-ui-manifest.mjs'
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
import { Switch } from '@/components/ui/switch'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { cn } from '@/lib/utils'
import type { Notify } from '@/app/route-context'
import { deleteComponent, setComponentEnabled, type ComponentRow } from './custom-ui-api'
import { LazyComponentHost, useComponentCatalog, useComponentsAvailable } from './placements'
import { customUiQueryKeys } from './query-keys'

type CustomUiPageProps = {
  embedded?: boolean
  notify: Notify
  // 路由里的选中组件；缺省时页面自己记住选中项（嵌入用法）。
  selectedId?: string
  // 选中或回到列表（id 为空）；replace 为真时替换当前历史记录（桌面双栏内切换）。
  onSelect?: (id: string, replace: boolean) => void
}

type Translate = ReturnType<typeof useI18n>['t']

// 列表只放原生组件与无法读取的目录：旧 iframe 组件已不再支持，不展示（负责人决定）。
function visibleRows(rows: readonly ComponentRow[]) {
  return rows.filter((row) => row.kind !== 'legacy')
}

function rowName(row: ComponentRow, language: string) {
  const i18n = row.manifest?.i18n as Record<string, { name?: string } | undefined> | undefined
  return i18n?.[language]?.name || row.name || row.id
}

// 错误码的静态文案表：check-i18n 只认 t() 的字面量键，所以逐个写出；没有文案的码原样显示。
function errorLabel(code: string, t: Translate) {
  const labels: Record<string, string> = {
    manifest_missing: t('custom-ui:errors.manifest_missing'),
    manifest_too_large: t('custom-ui:errors.manifest_too_large'),
    manifest_json_invalid: t('custom-ui:errors.manifest_json_invalid'),
    manifest_not_object: t('custom-ui:errors.manifest_not_object'),
    entry_missing: t('custom-ui:errors.entry_missing'),
    style_missing: t('custom-ui:errors.style_missing'),
    component_too_large: t('custom-ui:errors.component_too_large'),
  }
  return labels[code] || code
}

function statusLabel(row: ComponentRow, t: Translate) {
  if (row.kind === 'broken') return t('custom-ui:studio.statusBroken')
  if (row.status !== 'ready') return t('custom-ui:studio.statusInvalid')
  return row.enabled ? t('custom-ui:studio.statusEnabled') : t('custom-ui:studio.statusDisabled')
}

// 预览放置：manifest 声明的第一个放置位置（缺省视为四处都允许，取“功能页”整页）。
function previewSurface(row: ComponentRow): ComponentSurface {
  return row.manifest?.placements[0]?.surface ?? 'feature-page'
}

function previewBox(row: ComponentRow, surface: ComponentSurface) {
  const placement = row.manifest?.placements.find((entry) => entry.surface === surface)
  if (surface === 'overview-card') {
    const size = placement?.sizes?.[0] ?? 'S'
    return { size, ...OVERVIEW_CARD_DIMENSIONS[size] }
  }
  if (surface === 'floating') return { size: null, ...(placement?.size ?? FLOATING_DEFAULT_SIZE) }
  return { size: null, w: 0, h: 0 }
}

function ComponentDetail({
  row,
  notify,
  canDelete,
  phone = false,
  onDeleted,
}: {
  row: ComponentRow
  notify: Notify
  canDelete: boolean
  phone?: boolean
  onDeleted: () => void
}) {
  const { t, language } = useI18n()
  const client = useQueryClient()
  const [busy, setBusy] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const name = rowName(row, language)
  const surface = previewSurface(row)
  const box = previewBox(row, surface)
  const toggle = async (enabled: boolean) => {
    setBusy(true)
    try {
      await setComponentEnabled(row.id, enabled)
      await client.invalidateQueries({ queryKey: customUiQueryKeys.all })
    } catch {
      notify(t('custom-ui:studio.toggleFailed'))
    } finally {
      setBusy(false)
    }
  }
  const remove = async () => {
    setBusy(true)
    try {
      await deleteComponent(row.id)
      setConfirming(false)
      notify(t('custom-ui:studio.deleted', { name }))
      await client.invalidateQueries({ queryKey: customUiQueryKeys.all })
      onDeleted()
    } catch {
      notify(t('custom-ui:studio.deleteFailed'))
    } finally {
      setBusy(false)
    }
  }
  return (
    <section
      data-component-stage={row.id}
      data-component-kind={row.kind}
      className="flex min-h-0 min-w-0 flex-1 flex-col overflow-hidden rounded-md border border-stroke bg-panel"
    >
      <header className="flex min-w-0 flex-wrap items-center gap-2 border-b border-stroke-soft px-3.5 py-2.5">
        <strong className="min-w-0 flex-1 truncate text-[15px] font-medium">{name}</strong>
        {row.version && (
          <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
            v{row.version}
          </span>
        )}
        <span
          className={cn(
            'flex flex-wrap items-center gap-2',
            phone && 'grid w-full grid-cols-2 gap-2',
          )}
        >
          {row.kind === 'native' && (
            <label className="inline-flex min-h-8 items-center gap-2 text-xs text-content-secondary max-[650px]:min-h-11">
              <Switch
                checked={row.enabled}
                disabled={busy}
                aria-label={t('custom-ui:studio.enable')}
                data-component-enable
                onCheckedChange={(checked) => void toggle(checked)}
              />
              {t('custom-ui:studio.enable')}
            </label>
          )}
          {canDelete && (
            <Button
              variant="ghost"
              size="sm"
              className="max-[650px]:h-11"
              disabled={busy}
              data-component-delete
              onClick={() => setConfirming(true)}
            >
              <Trash2 />
              {t('custom-ui:studio.delete')}
            </Button>
          )}
        </span>
      </header>
      <div className="flex min-h-0 flex-1 flex-col gap-2.5 p-3.5">
        {row.description && (
          <p className="text-sm leading-relaxed text-content-secondary">{row.description}</p>
        )}
        <p className="text-xs leading-relaxed break-all text-content-muted">
          {t('custom-ui:studio.directory', { path: row.directory })}
        </p>
        {row.errors.length > 0 && (
          <ul
            role="alert"
            data-component-errors
            className="flex flex-col gap-1 rounded-md border border-stroke-soft bg-surface-subtle px-3 py-2 text-xs text-content-secondary"
          >
            {row.errors.map((error) => (
              <li key={`${error.path}:${error.code}`} className="min-w-0 break-all">
                {error.path && <code className="mr-1.5 font-mono">{error.path}</code>}
                {errorLabel(error.code, t)}
              </li>
            ))}
          </ul>
        )}
        {/* 预览舞台：凹陷的代码井底色 + 细线；总览卡片与浮窗按实际尺寸 1:1 渲染。 */}
        {row.kind === 'native' && row.status === 'ready' ? (
          <div
            data-component-preview
            className={cn(
              'flex min-h-[240px] flex-1 items-start justify-center overflow-auto rounded-md border border-stroke-soft bg-code p-3',
              phone && 'min-h-[min(420px,55dvh)]',
            )}
          >
            <div
              className="flex min-h-0 min-w-0 flex-col overflow-hidden rounded-md border border-stroke bg-panel"
              style={
                box.w
                  ? { width: box.w, maxWidth: '100%', height: box.h }
                  : { width: '100%', minHeight: 320, alignSelf: 'stretch' }
              }
            >
              <Suspense fallback={null}>
                <LazyComponentHost
                  // 按 rev 重挂：热重载后预览加载新模块（ComponentHost 自己也按 instanceId@rev 重挂）。
                  key={`${row.id}@${row.rev}`}
                  component={row}
                  instance={{
                    instanceId: `preview:${row.id}`,
                    surface,
                    props: {},
                    size: box.size,
                  }}
                  preview
                  notify={(message) => notify(message)}
                  className="h-full"
                />
              </Suspense>
            </div>
          </div>
        ) : (
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
        )}
      </div>
      <Dialog open={confirming} onOpenChange={(open) => !open && !busy && setConfirming(false)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t('custom-ui:studio.deleteTitle', { name })}</DialogTitle>
            <DialogDescription>{t('custom-ui:studio.deleteHint')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" disabled={busy} onClick={() => setConfirming(false)}>
              {t('custom-ui:studio.cancel')}
            </Button>
            <Button
              variant="destructive"
              disabled={busy}
              data-component-delete-confirm
              onClick={() => void remove()}
            >
              {t('custom-ui:studio.delete')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}

function RowButton({
  row,
  active,
  phone,
  onSelect,
}: {
  row: ComponentRow
  active: boolean
  phone: boolean
  onSelect: () => void
}) {
  const { t, language } = useI18n()
  const attention = row.status !== 'ready'
  return (
    <button
      type="button"
      data-component-row={row.id}
      data-component-kind={row.kind}
      aria-current={active || undefined}
      onClick={onSelect}
      className={cn(
        'flex w-full min-w-0 items-center gap-3 text-left transition-colors duration-(--d-fast) focus-visible:-outline-offset-2',
        phone
          ? 'min-h-14 px-3.5 py-2 hover:bg-surface-subtle active:bg-surface-subtle'
          : cn(
              'rounded-sm px-2.5 py-2 max-[650px]:min-h-11',
              active
                ? 'bg-surface-muted text-content'
                : 'text-content-secondary hover:bg-surface-subtle',
            ),
      )}
    >
      <span
        className={cn(
          'grid flex-none place-items-center rounded-md bg-surface-muted text-content-secondary ring-1 ring-stroke-soft ring-inset',
          phone ? 'size-9' : 'size-7',
        )}
      >
        <Blocks className={phone ? 'size-[18px]' : 'size-3.5'} strokeWidth={1.75} aria-hidden />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-center gap-2">
          <strong className="min-w-0 flex-1 truncate text-sm font-medium">
            {rowName(row, language)}
          </strong>
          {/* 需要处理（manifest 有错、目录无法读取）用中性徽标：琥珀只表示“需要你”的代理状态。 */}
          <Badge className={cn(attention && 'text-content-secondary')}>{statusLabel(row, t)}</Badge>
        </span>
        <small className="truncate font-mono text-xs text-content-muted">
          {row.version ? `${row.id} · v${row.version}` : row.id}
        </small>
      </span>
      {phone && <ChevronRight className="size-4 flex-none text-content-muted" aria-hidden="true" />}
    </button>
  )
}

export function CustomUiPage({
  notify,
  embedded = false,
  selectedId: routeSelectedId,
  onSelect,
}: CustomUiPageProps) {
  const { t } = useI18n()
  const phone = usePhoneShell() && !embedded
  const available = useComponentsAvailable()
  const catalog = useComponentCatalog()
  const rows = visibleRows(catalog.data?.components || [])
  const root = catalog.data?.root || ''
  const canDelete = catalog.data?.features.trash === 'move'
  const [localSelectedId, setLocalSelectedId] = useState('')
  const selectedId = onSelect ? routeSelectedId || '' : localSelectedId
  const setSelectedId = (id: string, replace = true) => {
    if (onSelect) onSelect(id, replace)
    else setLocalSelectedId(id)
  }
  const refreshing = catalog.isFetching
  const selected = rows.find((item) => item.id === selectedId) || rows[0] || null

  // 旧 Runtime（没有 components.native，或列表是旧形状）：如实说明不支持，不展示旧组件当成可用。
  if (!available || catalog.data?.legacyShape)
    return (
      <PageFrame variant="collection" embedded={embedded}>
        <PageState
          kind="empty"
          title={t('custom-ui:studio.unsupportedTitle')}
          description={t('custom-ui:studio.unsupportedHint')}
        />
      </PageFrame>
    )
  if (catalog.isPending)
    return <PageState kind="loading" title={t('custom-ui:customUiPage.loading')} />

  const error = catalog.error ? t('custom-ui:widget.catalogFailed') : ''
  const rescan = (
    <Button
      variant="ghost"
      size="icon"
      className={phone ? 'size-11' : 'size-8 max-[650px]:size-11'}
      title={t('custom-ui:customUiPage.rescan')}
      aria-label={t('custom-ui:customUiPage.rescan')}
      disabled={refreshing}
      onClick={() => void catalog.refetch()}
    >
      <RefreshCw className={cn('size-3.5', refreshing && 'animate-spin')} />
    </Button>
  )

  if (!rows.length && !error) {
    return (
      <PageFrame variant="collection" embedded={embedded}>
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
      </PageFrame>
    )
  }

  const errorNotice = error && (
    <AppNotice>
      <FolderCode size={14} />
      <span>{error}</span>
    </AppNotice>
  )

  // 手机：列表与详情各占一屏。详情屏顶部是“‹ 全部组件”，与手机返回键同一去向。
  if (phone) {
    const backToList = () => setSelectedId('', false)
    const detail = selectedId ? rows.find((item) => item.id === selectedId) || null : null
    if (selectedId)
      return (
        <PageFrame variant="collection">
          <div data-components-phone="detail" className="flex min-w-0 flex-col gap-3">
            <Button
              variant="ghost"
              className="-ml-2 min-h-11 w-fit px-2 text-content-secondary"
              onClick={backToList}
            >
              <ChevronLeft aria-hidden="true" />
              {t('custom-ui:phone.allComponents')}
            </Button>
            {detail ? (
              <ComponentDetail
                row={detail}
                notify={notify}
                canDelete={canDelete}
                phone
                onDeleted={backToList}
              />
            ) : (
              <PageState
                kind="empty"
                title={t('custom-ui:phone.missingTitle')}
                description={t('custom-ui:phone.missingHint')}
                action={
                  <Button variant="secondary" className="min-h-11" onClick={backToList}>
                    {t('custom-ui:phone.allComponents')}
                  </Button>
                }
              />
            )}
          </div>
        </PageFrame>
      )
    return (
      <PageFrame variant="collection">
        <div data-components-phone="list" className="flex min-w-0 flex-col gap-3">
          {errorNotice}
          <section
            aria-label={t('custom-ui:customUiPage.installedComponents', { count: rows.length })}
            className="flex flex-col overflow-hidden rounded-md border border-stroke bg-panel"
          >
            <header className="flex min-h-11 items-center justify-between gap-2 border-b border-stroke-soft bg-surface-subtle pl-3.5">
              <span className="text-xs font-medium text-content-secondary tabular-nums">
                {t('custom-ui:customUiPage.installedComponents', { count: rows.length })}
              </span>
              {rescan}
            </header>
            <ul className="flex flex-col divide-y divide-stroke-soft">
              {rows.map((row) => (
                <li key={row.id}>
                  <RowButton
                    row={row}
                    active={false}
                    phone
                    onSelect={() => setSelectedId(row.id, false)}
                  />
                </li>
              ))}
            </ul>
          </section>
          <p className="text-xs leading-relaxed break-all text-content-muted">
            {t('custom-ui:customUiPage.directoryHint', { path: root })}
          </p>
        </div>
      </PageFrame>
    )
  }

  return (
    <PageFrame variant="collection" embedded={embedded}>
      <div className="flex min-w-0 flex-col gap-3">
        {errorNotice}
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-3 lg:grid-cols-[minmax(220px,280px)_minmax(0,1fr)]">
          <section className="flex min-h-0 flex-col overflow-hidden rounded-md border border-stroke bg-panel">
            <header className="flex min-h-10 items-center justify-between gap-2 border-b border-stroke-soft bg-surface-subtle py-1 pr-1.5 pl-3.5">
              <span className="text-xs font-medium text-content-secondary tabular-nums">
                {t('custom-ui:customUiPage.installedComponents', { count: rows.length })}
              </span>
              {rescan}
            </header>
            <ul className="flex min-h-0 flex-col gap-0.5 overflow-y-auto p-1.5">
              {rows.map((row) => {
                const active = row.id === selected?.id
                return (
                  <li key={row.id} className="relative">
                    {active && (
                      <i
                        aria-hidden="true"
                        className="absolute inset-y-2 left-0 w-[3px] rounded-r-[3px] bg-content-muted"
                      />
                    )}
                    <RowButton
                      row={row}
                      active={active}
                      phone={false}
                      onSelect={() => setSelectedId(row.id)}
                    />
                  </li>
                )
              })}
            </ul>
            <p className="mt-auto border-t border-stroke-soft px-3.5 py-2.5 text-xs leading-relaxed break-all text-content-muted">
              {t('custom-ui:customUiPage.directoryHint', { path: root })}
            </p>
          </section>
          {selected ? (
            <ComponentDetail
              key={selected.id}
              row={selected}
              notify={notify}
              canDelete={canDelete}
              onDeleted={() => setSelectedId('')}
            />
          ) : (
            <InlineEmptyState title={t('custom-ui:customUiPage.selectComponent')} />
          )}
        </div>
      </div>
    </PageFrame>
  )
}
