// 工作室右栏 · 设置（规格 5.1、5.3–5.5、5.9）：身份行与 [启用]、属性（按放置实例分别保存）、使用的能力
// （A8 起带调用台账与 [撤销]）、放置位置（四个平级磁贴）、版本（A8：[对比] [回滚]）、
// 吸底操作栏（[放置到… ▾] · [查看源码] · [导出 ZIP] · ⋯ 删除组件）。
// 窗口 <1440 或侧栏展开时整栏改放进右侧抽屉，内容不变；手机详情按“预览 · 属性 · 能力 · 放置 · 版本”顺序复用各段。
import { useRef, useState } from 'react'
import {
  AppWindow,
  Blocks,
  ChevronDown,
  Code2,
  Download,
  LayoutDashboard,
  MoreHorizontal,
  PanelRight,
  Plus,
  Trash2,
} from 'lucide-react'
import type {
  CapabilityType,
  ComponentSurface,
  OverviewCardSize,
} from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SegmentedControl } from '@/components/ui/segmented-control'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { useOverlayFocusReturn } from '@/hooks/useOverlayFocusReturn'
import { relativeTime } from '@/lib/format'
import { cn } from '@/lib/utils'
import { useBoardSessions } from '@/features/board/public'
import type {
  ComponentCatalogFeatures,
  ComponentInstance,
  ComponentInstanceScope,
  ComponentRow,
} from '@/features/custom-ui/custom-ui-api'
import { componentErrorText } from './error-text'
import { PropsForm, type PreviewDraft } from './PropsForm'
import { capabilityLabel, surfaceLabel } from './labels'
import { StudioSection } from './StudioSection'
import { canPlace, componentDisplayName, defaultCardSize, surfaceDeclared } from './studio-model'
import type { useStudioActions } from './use-studio-actions'
import { useComponentUsage, type LifecycleActions } from './use-lifecycle'
import { VersionsSection } from './VersionsSection'

type Translate = ReturnType<typeof useI18n>['t']
type StudioActions = ReturnType<typeof useStudioActions>

const SURFACES: readonly ComponentSurface[] = [
  'overview-card',
  'session-panel',
  'feature-page',
  'floating',
]
const SURFACE_ICONS = {
  'overview-card': LayoutDashboard,
  'session-panel': PanelRight,
  'feature-page': AppWindow,
  floating: Blocks,
} as const

function sourceLabel(row: ComponentRow, t: Translate) {
  if (row.source === 'agent') return t('custom-ui:studio.sourceAgent')
  if (row.source === 'template') return t('custom-ui:studio.sourceTemplate')
  if (row.source === 'import') return t('custom-ui:studio.sourceImport')
  return t('custom-ui:studio.sourceLocal')
}

function instanceLabel(instance: ComponentInstance, t: Translate) {
  const surface = surfaceLabel(instance.surface, t)
  const base = instance.size ? `${surface} ${instance.size}` : surface
  return instance.title ? `${base} · ${instance.title}` : base
}

function projectName(cwd: string) {
  const parts = cwd.replace(/[\\/]+$/, '').split(/[\\/]/)
  return parts[parts.length - 1] || cwd
}

export function IdentityRow({ row, actions }: { row: ComponentRow; actions: StudioActions }) {
  const { t, language } = useI18n()
  const [busy, setBusy] = useState(false)
  return (
    <div className="flex min-w-0 items-center gap-3">
      <span className="grid size-10 flex-none place-items-center rounded-[var(--r-md)] border border-stroke bg-solid text-content-secondary">
        <Blocks className="size-5" strokeWidth={1.75} aria-hidden />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <strong className="truncate text-[15px] font-semibold">
          {componentDisplayName(row, language)}
        </strong>
        <span className="truncate text-xs text-content-muted">
          {[
            row.version ? `v${row.version}` : '',
            row.kind === 'broken'
              ? t('custom-ui:studio.statusBroken')
              : t('custom-ui:studio.nativeComponent'),
            row.kind === 'native' ? sourceLabel(row, t) : '',
          ]
            .filter(Boolean)
            .join(' · ')}
        </span>
      </span>
      {row.kind === 'native' && (
        <label className="inline-flex flex-none items-center gap-2 text-xs text-content-secondary">
          {t('custom-ui:studio.enable')}
          <Switch
            checked={row.enabled}
            disabled={busy}
            aria-label={t('custom-ui:studio.enable')}
            data-component-enable
            onCheckedChange={async (checked) => {
              setBusy(true)
              await actions.setEnabled(row, checked)
              setBusy(false)
            }}
          />
        </label>
      )}
    </div>
  )
}

export function ErrorList({ row }: { row: ComponentRow }) {
  const { t } = useI18n()
  if (!row.errors.length) return null
  return (
    // 静态错误清单（选中一个有错的组件就会出现）不用 alert：读屏会把它当紧急通知打断朗读。
    <ul
      data-component-errors
      className="flex flex-col gap-1 rounded-[var(--r-md)] border border-stroke bg-surface-subtle px-3 py-2 text-xs text-content-secondary"
    >
      {row.errors.map((error) => (
        <li key={`${error.path}:${error.code}`} className="min-w-0 break-all">
          {error.path && <code className="mr-1.5 font-mono">{error.path}</code>}
          {componentErrorText(error.code, t, error.params)}
        </li>
      ))}
    </ul>
  )
}

export function PropsSection({
  row,
  instances,
  selectedInstanceId,
  notify,
  actions,
  onSelectInstance,
  onPreview,
}: {
  row: ComponentRow
  instances: readonly ComponentInstance[]
  selectedInstanceId: string | null
  notify: (message: string) => void
  actions: StudioActions
  onSelectInstance: (instanceId: string | null) => void
  onPreview: (draft: PreviewDraft) => void
}) {
  const { t } = useI18n()
  const own = instances.filter((instance) => instance.componentId === row.id)
  const selected = own.find((instance) => instance.instanceId === selectedInstanceId) ?? null
  return (
    <StudioSection
      title={t('custom-ui:studio.props')}
      aside={t('custom-ui:studio.propsPerInstance')}
    >
      {own.length > 0 && (
        <Select
          value={selected?.instanceId ?? 'preview'}
          onValueChange={(value) => onSelectInstance(value === 'preview' ? null : value)}
        >
          <SelectTrigger
            className="w-full"
            aria-label={t('custom-ui:studio.editingInstance')}
            data-props-instance
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="preview">{t('custom-ui:studio.previewProps')}</SelectItem>
            {own.map((instance) => (
              <SelectItem key={instance.instanceId} value={instance.instanceId}>
                {instanceLabel(instance, t)}
                {!instance.enabled && ` · ${t('custom-ui:studio.instanceClosed')}`}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      <PropsForm
        key={selected?.instanceId ?? `preview:${row.id}`}
        row={row}
        instance={selected}
        notify={notify}
        onPreview={onPreview}
        onSave={actions.saveProps}
      />
    </StudioSection>
  )
}

/**
 * 使用的能力（规格 5.4）：声明清单，每行是能力名、目标、最近一次调用与今天的次数（A8 台账），以及 [撤销]。
 * 撤销后 ctx 不再提供这项能力，网络与存储在 Runtime 代理处直接拒绝；可以随时 [恢复]。
 * 末尾一行列出 Pisper 接口不会提供的能力，固定说明写明这不是隔离边界。
 */
export function CapabilitiesSection({
  row,
  usageEnabled = false,
  canRevoke = false,
  lifecycle,
}: {
  row: ComponentRow
  usageEnabled?: boolean
  canRevoke?: boolean
  lifecycle?: LifecycleActions
}) {
  const { t, language } = useI18n()
  const capabilities = row.manifest?.capabilities ?? {}
  const usage = useComponentUsage(row.id, usageEnabled)
  const [busy, setBusy] = useState('')
  const declared = (
    ['network', 'notify', 'sessions.read', 'sessions.open', 'storage'] as const
  ).filter((type) => capabilities[type])
  const undeclared = (['sessions.read', 'notify', 'network', 'storage'] as const)
    .filter((type) => !capabilities[type])
    .map((type) => capabilityLabel(type, t))
  const notify = lifecycle?.notify ?? (() => undefined)
  const toggle = async (type: CapabilityType, revoke: boolean) => {
    if (!lifecycle) return
    setBusy(type)
    const next = revoke
      ? [...new Set([...row.revoked, type])]
      : row.revoked.filter((entry) => entry !== type)
    const result = await lifecycle.setRevoked(row, next)
    setBusy('')
    if (result)
      notify(
        revoke
          ? t('custom-ui:usage.revoked', { capability: capabilityLabel(type, t) })
          : t('custom-ui:usage.unrevoked', { capability: capabilityLabel(type, t) }),
      )
  }
  return (
    <StudioSection title={t('custom-ui:studio.capabilities')}>
      {declared.length ? (
        <ul
          data-component-capabilities
          className="overflow-hidden rounded-[var(--r-md)] border border-stroke"
        >
          {declared.map((type) => {
            const entry = capabilities[type]
            const revoked = row.revoked.includes(type)
            const used = usage.data?.usage[type]
            const detail =
              type === 'network'
                ? capabilities.network?.hosts.join(', ')
                : type === 'storage'
                  ? t('custom-ui:studio.capStorageQuota', {
                      kb: capabilities.storage?.quotaKb ?? 0,
                    })
                  : ''
            return (
              <li
                key={type}
                data-component-capability={type}
                data-capability-revoked={revoked || undefined}
                className="flex min-w-0 items-start gap-2 border-b border-stroke px-3 py-2 last:border-b-0"
              >
                <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                  <span className="flex min-w-0 items-baseline gap-2 text-sm">
                    <span className={cn(revoked && 'text-content-muted line-through')}>
                      {capabilityLabel(type, t)}
                    </span>
                    {detail && (
                      <code className="min-w-0 truncate font-mono text-xs text-content-secondary">
                        {detail}
                      </code>
                    )}
                    {revoked && (
                      <Badge className="flex-none text-content-secondary">
                        {t('custom-ui:usage.revokedBadge')}
                      </Badge>
                    )}
                  </span>
                  {entry?.reason && (
                    <span className="text-xs text-content-muted">{entry.reason}</span>
                  )}
                  {usageEnabled && (
                    <span className="text-xs text-content-muted" data-capability-usage>
                      {used
                        ? [
                            t('custom-ui:usage.last', {
                              time: relativeTime(used.lastAt, language),
                            }),
                            used.target,
                            t('custom-ui:usage.today', { count: used.today }),
                          ]
                            .filter(Boolean)
                            .join(' · ')
                        : t('custom-ui:usage.never')}
                    </span>
                  )}
                </span>
                {canRevoke && lifecycle && (
                  <Button
                    size="xs"
                    variant="ghost"
                    disabled={busy === type}
                    data-capability-toggle={type}
                    className="flex-none"
                    onClick={() => void toggle(type, !revoked)}
                  >
                    {revoked ? t('custom-ui:usage.restore') : t('custom-ui:usage.revoke')}
                  </Button>
                )}
              </li>
            )
          })}
        </ul>
      ) : (
        <p className="text-xs text-content-muted">{t('custom-ui:studio.capNone')}</p>
      )}
      <p className="text-xs text-content-muted">
        {t('custom-ui:studio.capUndeclared', {
          list: [
            ...undeclared,
            t('custom-ui:studio.capFiles'),
            t('custom-ui:studio.capClipboard'),
          ].join(' · '),
        })}
      </p>
      <p className="text-xs leading-relaxed text-content-muted">
        {t('custom-ui:studio.capExplainer')}
      </p>
    </StudioSection>
  )
}

function ScopeSelect({
  value,
  disabled,
  onChange,
}: {
  value: ComponentInstanceScope
  disabled: boolean
  onChange: (scope: ComponentInstanceScope) => void
}) {
  const { t } = useI18n()
  // 项目来自全部代理的数据（侧栏本来就在同步，不额外启动请求）；只列有目录的项目。
  const { projects } = useBoardSessions()
  const options = projects.filter((project) => project.cwd)
  const current = value.type === 'project' ? value.cwd : '*'
  if (value.type === 'project' && !options.some((project) => project.cwd === value.cwd))
    options.push({ key: value.cwd, cwd: value.cwd, count: 0 })
  return (
    <Select
      value={current}
      disabled={disabled}
      onValueChange={(next) =>
        onChange(next === '*' ? { type: 'all' } : { type: 'project', cwd: next })
      }
    >
      <SelectTrigger
        size="sm"
        className="w-full min-w-0"
        aria-label={t('custom-ui:studio.scope')}
        data-placement-scope
      >
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="*">{t('custom-ui:studio.scopeAll')}</SelectItem>
        {options.map((project) => (
          <SelectItem key={project.cwd} value={project.cwd}>
            {t('custom-ui:studio.scopeProject', { name: projectName(project.cwd) })}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )
}

export function PlacementSection({
  row,
  instances,
  actions,
}: {
  row: ComponentRow
  instances: readonly ComponentInstance[]
  actions: StudioActions
}) {
  const { t } = useI18n()
  const [busy, setBusy] = useState(false)
  const [cardSize, setCardSize] = useState<OverviewCardSize>(defaultCardSize(row.manifest))
  const [scope, setScope] = useState<ComponentInstanceScope>({ type: 'all' })
  const [confirmRemove, setConfirmRemove] = useState<ComponentInstance[] | null>(null)
  // 确认框由磁贴的勾选框打开（没有 DialogTrigger）：关闭后把焦点还给它，键盘与读屏不丢位置。
  const returnRemoveFocus = useOverlayFocusReturn(confirmRemove !== null)
  const placeable = canPlace(row)
  const own = instances.filter((instance) => instance.componentId === row.id)
  const declaredSizes =
    row.manifest?.placements.find((entry) => entry.surface === 'overview-card')?.sizes ??
    (['S', 'M'] as const)
  const step = async (task: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await task()
    } finally {
      setBusy(false)
    }
  }
  return (
    <StudioSection
      title={t('custom-ui:studio.placements')}
      aside={
        !placeable && row.kind === 'native' ? t('custom-ui:studio.placeNeedsEnable') : undefined
      }
    >
      <ul className="grid grid-cols-1 gap-2 min-[360px]:grid-cols-2" data-component-placements>
        {SURFACES.map((surface) => {
          const Icon = SURFACE_ICONS[surface]
          const placed = own.filter((instance) => instance.surface === surface)
          const active = placed.filter((instance) => instance.enabled)
          const checked = active.length > 0
          const declared = surfaceDeclared(row.manifest, surface)
          const disabled = busy || !placeable || !declared
          const single = surface === 'overview-card' && active.length === 1 ? active[0] : null
          const toggle = () => {
            if (checked) {
              if (active.length > 1) setConfirmRemove(active)
              else void step(() => actions.unplace(active[0]))
            } else
              void step(() =>
                actions.place(row, instances, {
                  surface,
                  ...(surface === 'overview-card' ? { size: cardSize } : {}),
                  ...(surface === 'session-panel' ? { scope } : {}),
                }),
              )
          }
          const panel = placed[0]
          return (
            <li
              key={surface}
              data-placement-tile={surface}
              data-placement-checked={checked || undefined}
              className={cn(
                'flex min-w-0 flex-col gap-2 rounded-[var(--r-md)] border px-2.5 py-2',
                checked ? 'border-stroke-strong bg-surface-subtle' : 'border-stroke',
                !declared && 'opacity-60',
              )}
            >
              <label className="flex min-w-0 items-center gap-2 text-sm pointer-coarse:min-h-11">
                <Checkbox
                  checked={checked}
                  disabled={disabled}
                  aria-describedby={!declared ? `placement-${surface}-hint` : undefined}
                  onCheckedChange={toggle}
                />
                <Icon className="size-3.5 flex-none text-content-muted" aria-hidden />
                <span className="min-w-0 flex-1 truncate">{surfaceLabel(surface, t)}</span>
                {surface === 'overview-card' && active.length > 1 && (
                  <Badge className="flex-none">×{active.length}</Badge>
                )}
              </label>
              {!declared && (
                <span id={`placement-${surface}-hint`} className="text-xs text-content-muted">
                  {t('custom-ui:studio.placeUndeclared')}
                </span>
              )}
              {declared && surface === 'overview-card' && (
                <span className="flex items-center gap-1.5">
                  {declaredSizes.length > 1 && (
                    <SegmentedControl<OverviewCardSize>
                      size="sm"
                      aria-label={t('custom-ui:studio.cardSize')}
                      disabled={disabled}
                      value={single?.size ?? cardSize}
                      onValueChange={(size) => {
                        setCardSize(size)
                        if (single) void step(() => actions.patchInstance(single, { size }))
                      }}
                      options={declaredSizes.map((size) => ({ value: size, label: size }))}
                    />
                  )}
                  {checked && (
                    <Button
                      size="xs"
                      variant="ghost"
                      disabled={disabled}
                      title={t('custom-ui:studio.addCard')}
                      aria-label={t('custom-ui:studio.addCard')}
                      onClick={() =>
                        void step(() => actions.place(row, instances, { surface, size: cardSize }))
                      }
                    >
                      <Plus />
                    </Button>
                  )}
                </span>
              )}
              {declared && surface === 'session-panel' && (
                <ScopeSelect
                  value={panel?.scope ?? scope}
                  disabled={disabled}
                  onChange={(next) => {
                    setScope(next)
                    if (panel) void step(() => actions.patchInstance(panel, { scope: next }))
                  }}
                />
              )}
              {declared && surface === 'floating' && (
                <span className="text-xs text-content-muted">
                  {t('custom-ui:studio.floatingPhoneHint')}
                </span>
              )}
            </li>
          )
        })}
      </ul>
      <Dialog
        open={confirmRemove !== null}
        onOpenChange={(open) => !open && setConfirmRemove(null)}
      >
        <DialogContent onCloseAutoFocus={returnRemoveFocus}>
          <DialogHeader>
            <DialogTitle>
              {t('custom-ui:studio.unplaceAllTitle', { count: confirmRemove?.length ?? 0 })}
            </DialogTitle>
            <DialogDescription>{t('custom-ui:studio.unplaceAllHint')}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmRemove(null)}>
              {t('custom-ui:studio.cancel')}
            </Button>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() => {
                const targets = confirmRemove ?? []
                setConfirmRemove(null)
                void step(async () => {
                  for (const instance of targets) await actions.unplace(instance)
                })
              }}
            >
              {t('custom-ui:studio.unplaceAll')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </StudioSection>
  )
}

// [放置到… ▾]：四处平级（规格 5.5，没有偏向某一处的主按钮）；总览每次加一张，其余已放置的项打勾置灰。
export function PlaceMenu({
  row,
  instances,
  actions,
  className,
}: {
  row: ComponentRow
  instances: readonly ComponentInstance[]
  actions: StudioActions
  className?: string
}) {
  const { t } = useI18n()
  const placeable = canPlace(row)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="sm" disabled={!placeable} data-component-place className={className}>
          {t('custom-ui:studio.placeTo')}
          <ChevronDown />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        {SURFACES.map((surface) => {
          const declared = surfaceDeclared(row.manifest, surface)
          const placed =
            surface !== 'overview-card' &&
            instances.some(
              (instance) =>
                instance.componentId === row.id && instance.surface === surface && instance.enabled,
            )
          return (
            <DropdownMenuItem
              key={surface}
              disabled={!declared || placed}
              data-component-place-item={surface}
              onSelect={() =>
                void actions.place(row, instances, {
                  surface,
                  ...(surface === 'overview-card' ? { size: defaultCardSize(row.manifest) } : {}),
                })
              }
            >
              {surfaceLabel(surface, t)}
              <span className="ml-auto text-xs text-content-muted">
                {!declared
                  ? t('custom-ui:studio.placeUndeclaredShort')
                  : placed
                    ? t('custom-ui:studio.placed')
                    : ''}
              </span>
            </DropdownMenuItem>
          )
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

export function DeleteComponentButton({
  row,
  actions,
  onDeleted,
  variant = 'menu',
}: {
  row: ComponentRow
  actions: StudioActions
  onDeleted: () => void
  variant?: 'menu' | 'button'
}) {
  const { t, language } = useI18n()
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  // 从 ⋯ 菜单项打开时记下的菜单项会随菜单卸载：兜底还给 ⋯ 按钮（删除成功后行没了，就不归还）。
  const moreRef = useRef<HTMLButtonElement>(null)
  const returnFocus = useOverlayFocusReturn(confirming, moreRef)
  const name = componentDisplayName(row, language)
  const remove = async () => {
    setBusy(true)
    const result = await actions.remove(row)
    setBusy(false)
    if (!result) return
    setConfirming(false)
    onDeleted()
  }
  return (
    <>
      {variant === 'menu' ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              ref={moreRef}
              size="icon-sm"
              variant="ghost"
              aria-label={t('custom-ui:studio.more')}
              data-component-more
            >
              <MoreHorizontal />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem
              variant="destructive"
              data-component-delete
              onSelect={() => setConfirming(true)}
            >
              <Trash2 />
              {t('custom-ui:studio.deleteComponent')}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      ) : (
        <Button
          variant="ghost"
          size="sm"
          className="text-danger"
          data-component-delete
          onClick={() => setConfirming(true)}
        >
          <Trash2 />
          {t('custom-ui:studio.deleteComponent')}
        </Button>
      )}
      <Dialog open={confirming} onOpenChange={(open) => !open && !busy && setConfirming(false)}>
        <DialogContent onCloseAutoFocus={returnFocus}>
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
    </>
  )
}

/** [导出 ZIP]（规格 5.9）：manifest、入口与资源，不含实例与存储数据。 */
export function ExportComponentButton({
  row,
  lifecycle,
  className,
}: {
  row: ComponentRow
  lifecycle: LifecycleActions
  className?: string
}) {
  const { t } = useI18n()
  const [busy, setBusy] = useState(false)
  return (
    <Button
      size="sm"
      variant="secondary"
      disabled={busy}
      data-component-export
      className={className}
      onClick={async () => {
        setBusy(true)
        await lifecycle.exportZip(row)
        setBusy(false)
      }}
    >
      <Download />
      {t('custom-ui:exporting.button')}
    </Button>
  )
}

export function StudioSettings({
  row,
  instances,
  selectedInstanceId,
  canDelete,
  features,
  notify,
  actions,
  lifecycle,
  onSelectInstance,
  onPreview,
  onViewSource,
  onDeleted,
}: {
  row: ComponentRow
  instances: readonly ComponentInstance[]
  selectedInstanceId: string | null
  canDelete: boolean
  features: ComponentCatalogFeatures
  notify: (message: string) => void
  actions: StudioActions
  lifecycle: LifecycleActions
  onSelectInstance: (instanceId: string | null) => void
  onPreview: (draft: PreviewDraft) => void
  onViewSource: () => void
  onDeleted: () => void
}) {
  const { t } = useI18n()
  const runnable = row.kind === 'native' && row.status === 'ready' && Boolean(row.manifest)
  return (
    <div data-studio-settings className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-0 flex-1 flex-col gap-5 overflow-y-auto px-4 pt-4 pb-5">
        <IdentityRow row={row} actions={actions} />
        <ErrorList row={row} />
        {runnable && (
          <>
            <PropsSection
              row={row}
              instances={instances}
              selectedInstanceId={selectedInstanceId}
              notify={notify}
              actions={actions}
              onSelectInstance={onSelectInstance}
              onPreview={onPreview}
            />
            <CapabilitiesSection
              row={row}
              usageEnabled={features.usage}
              canRevoke={features.revoke}
              lifecycle={lifecycle}
            />
            <PlacementSection row={row} instances={instances} actions={actions} />
          </>
        )}
        {/* 版本对 manifest 有错的原生组件同样有用：回滚往往就是修好它的最快办法。 */}
        {row.kind === 'native' && (
          <VersionsSection
            row={row}
            enabled={features.versions}
            notify={notify}
            actions={lifecycle}
          />
        )}
      </div>
      <footer className="flex flex-none items-center gap-2 border-t border-stroke bg-panel px-4 py-2.5">
        {runnable && <PlaceMenu row={row} instances={instances} actions={actions} />}
        <Button size="sm" variant="secondary" onClick={onViewSource} data-component-view-source>
          <Code2 />
          {t('custom-ui:studio.viewSource')}
        </Button>
        {features.export && row.kind === 'native' && (
          <ExportComponentButton row={row} lifecycle={lifecycle} />
        )}
        {canDelete && (
          <span className="ml-auto">
            <DeleteComponentButton row={row} actions={actions} onDeleted={onDeleted} />
          </span>
        )}
      </footer>
    </div>
  )
}
