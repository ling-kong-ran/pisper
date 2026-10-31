// 工作室左栏 · 组件库（规格 5.1）：数量、[导入 ZIP]（A7 置灰，A8 的暂存导入）、[新建 ▾]、搜索、
// 筛选（全部 / 已启用 / 需处理），每行图标 · 名称 · 版本 · 放置摘要；选中行用 --active 底（不用琥珀）。
// 本页崩溃过或被暂停的组件在行下原地展开错误条；底部是热重载目录。手机列表复用同一行。
import { useSyncExternalStore, type ReactNode } from 'react'
import { Blocks, ChevronRight, FileArchive, Search } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { SegmentedControl } from '@/components/ui/segmented-control'
import { cn } from '@/lib/utils'
import type { ComponentInstance, ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { componentLogBuffer } from '@/features/custom-ui/native/host-runtime'
import { NewComponentMenu } from './NewComponentMenu'
import {
  componentDisplayName,
  placementSummary,
  type PlacementSummary,
  type StudioFilter,
} from './studio-model'
import { componentErrorText } from './error-text'
import { surfaceLabel } from './labels'
import type { StudioTemplate } from './use-studio-actions'

type Translate = ReturnType<typeof useI18n>['t']

function summaryText(parts: readonly PlacementSummary[], t: Translate) {
  if (!parts.length) return t('custom-ui:studio.notPlaced')
  return parts
    .map((part) => {
      const label = surfaceLabel(part.surface, t)
      if (part.surface !== 'overview-card') return label
      const sizes = part.sizes.join('/')
      const text = sizes ? `${label} ${sizes}` : label
      return part.count > 1 ? `${text} ×${part.count}` : text
    })
    .join(' · ')
}

// 行的第二行：可运行的组件显示放置摘要；有问题的显示原因（第一条错误码的文案）。
function rowDetail(row: ComponentRow, instances: readonly ComponentInstance[], t: Translate) {
  if (row.kind === 'broken' || row.status !== 'ready') {
    const first = row.errors[0]
    return first
      ? componentErrorText(first.code, t, first.params)
      : t('custom-ui:studio.statusInvalid')
  }
  return summaryText(placementSummary(row.id, instances), t)
}

function rowBadge(row: ComponentRow, t: Translate): string | null {
  if (row.kind === 'broken') return t('custom-ui:studio.statusBroken')
  if (row.status !== 'ready') return t('custom-ui:studio.statusInvalid')
  if (!row.enabled) return t('custom-ui:studio.statusDisabled')
  return null
}

export function StudioRow({
  row,
  instances,
  active,
  phone = false,
  onSelect,
}: {
  row: ComponentRow
  instances: readonly ComponentInstance[]
  active: boolean
  phone?: boolean
  onSelect: () => void
}) {
  const { t, language } = useI18n()
  const badge = rowBadge(row, t)
  return (
    <button
      type="button"
      data-component-row={row.id}
      data-component-kind={row.kind}
      aria-current={active || undefined}
      onClick={onSelect}
      className={cn(
        'flex w-full min-w-0 items-center gap-3 text-left transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
        phone
          ? 'min-h-14 px-3.5 py-2 hover:bg-surface-hover active:bg-surface-hover'
          : cn(
              'rounded-[var(--r-md)] px-2.5 py-2',
              active ? 'bg-surface-active' : 'hover:bg-surface-hover',
            ),
      )}
    >
      <span
        className={cn(
          'grid flex-none place-items-center rounded-[var(--r-md)] border border-stroke bg-solid text-content-secondary',
          phone ? 'size-9' : 'size-8',
        )}
      >
        <Blocks className="size-4" strokeWidth={1.75} aria-hidden />
      </span>
      <span className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex min-w-0 items-center gap-1.5">
          <strong className="min-w-0 truncate text-sm font-medium text-content">
            {componentDisplayName(row, language)}
          </strong>
          {row.version && (
            <span className="flex-none font-mono text-xs text-content-muted tabular-nums">
              v{row.version}
            </span>
          )}
          {/* 需要处理（manifest 有错、目录无法读取）与已停用都用中性徽标：琥珀只表示“需要你”。 */}
          {badge && <Badge className="ml-auto flex-none text-content-secondary">{badge}</Badge>}
        </span>
        <small className="truncate text-xs text-content-muted" data-component-row-detail>
          {rowDetail(row, instances, t)}
        </small>
      </span>
      {phone && <ChevronRight className="size-4 flex-none text-content-muted" aria-hidden="true" />}
    </button>
  )
}

// 本页崩溃过 / 被暂停的组件：行下原地展开的错误条，带最近一条错误与两个动作。
function CrashStrip({
  componentId,
  onOpenLogs,
  onReload,
}: {
  componentId: string
  onOpenLogs: () => void
  onReload: () => void
}) {
  const { t } = useI18n()
  const buffer = componentLogBuffer()
  const message = useSyncExternalStore(
    (listener) => buffer.subscribe(listener),
    () => {
      const entries = buffer.read(componentId)
      for (let index = entries.length - 1; index >= 0; index -= 1)
        if (entries[index].level === 'error') return entries[index].message
      return ''
    },
  )
  return (
    <div
      role="alert"
      data-component-crash={componentId}
      className="mx-1 mt-1 mb-1.5 flex flex-col gap-1.5 rounded-[var(--r-md)] border border-stroke bg-surface-subtle px-2.5 py-2"
    >
      <p className="flex items-center gap-1.5 text-xs font-medium text-st-fail">
        <span aria-hidden className="size-1.5 rounded-full bg-st-fail" />
        {t('custom-ui:host.renderErrorTitle')}
      </p>
      {message && (
        <p className="truncate font-mono text-xs text-content-secondary" title={message}>
          {message}
        </p>
      )}
      <div className="flex flex-wrap gap-1">
        <Button size="xs" variant="ghost" onClick={onOpenLogs}>
          {t('custom-ui:host.viewError')}
        </Button>
        <Button size="xs" variant="secondary" onClick={onReload}>
          {t('custom-ui:host.reload')}
        </Button>
      </div>
    </div>
  )
}

export function StudioImportButton({ className }: { className?: string }) {
  const { t } = useI18n()
  // 导入（暂存 + 信任确认）在 A8：现在置灰并说明，按钮本身仍可聚焦读出原因。
  return (
    <Button
      size="sm"
      variant="ghost"
      aria-disabled="true"
      title={t('custom-ui:studio.importLater')}
      aria-label={`${t('custom-ui:studio.importZip')} · ${t('custom-ui:studio.importLater')}`}
      data-component-import
      className={className}
      onClick={(event) => event.preventDefault()}
    >
      <FileArchive />
      {t('custom-ui:studio.importZip')}
    </Button>
  )
}

export function StudioLibrary({
  rows,
  allCount,
  counts,
  instances,
  selectedId,
  troubled,
  filter,
  query,
  root,
  onFilter,
  onQuery,
  onSelect,
  onCreate,
  onOpenLogs,
  onReload,
  footer,
}: {
  rows: readonly ComponentRow[]
  allCount: number
  counts: { all: number; enabled: number; attention: number }
  instances: readonly ComponentInstance[]
  selectedId: string
  troubled: ReadonlySet<string>
  filter: StudioFilter
  query: string
  root: string
  onFilter: (filter: StudioFilter) => void
  onQuery: (query: string) => void
  onSelect: (id: string) => void
  onCreate: (template: StudioTemplate, name?: string) => Promise<ComponentRow | null>
  onOpenLogs: (id: string) => void
  onReload: (id: string) => void
  footer?: ReactNode
}) {
  const { t } = useI18n()
  return (
    <section
      data-studio-library
      aria-label={t('custom-ui:studio.library')}
      className="flex min-h-0 min-w-0 flex-col border-r border-stroke"
    >
      <header className="flex flex-col gap-2.5 px-3 pt-3.5 pb-2.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <h2 className="mr-auto flex min-w-0 items-baseline gap-1.5 truncate text-base font-semibold">
            {t('custom-ui:studio.libraryTitle')}
            <span className="text-sm font-normal text-content-muted tabular-nums">{allCount}</span>
          </h2>
          <StudioImportButton />
          <NewComponentMenu onCreate={onCreate} />
        </div>
        <label className="relative block">
          <Search
            aria-hidden
            className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-content-muted"
          />
          <Input
            type="search"
            value={query}
            placeholder={t('custom-ui:studio.search')}
            aria-label={t('custom-ui:studio.search')}
            className="pl-8"
            onChange={(event) => onQuery(event.target.value)}
          />
        </label>
        <SegmentedControl<StudioFilter>
          size="sm"
          aria-label={t('custom-ui:studio.filter')}
          value={filter}
          onValueChange={onFilter}
          className="w-full [&>*]:flex-1"
          options={[
            { value: 'all', label: t('custom-ui:studio.filterAll') },
            { value: 'enabled', label: t('custom-ui:studio.filterEnabled'), count: counts.enabled },
            {
              value: 'attention',
              label: t('custom-ui:studio.filterAttention'),
              count: counts.attention,
            },
          ]}
        />
      </header>
      <ul className="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto px-2 pb-2">
        {rows.map((row) => (
          <li key={row.id}>
            <StudioRow
              row={row}
              instances={instances}
              active={row.id === selectedId}
              onSelect={() => onSelect(row.id)}
            />
            {troubled.has(row.id) && (
              <CrashStrip
                componentId={row.id}
                onOpenLogs={() => onOpenLogs(row.id)}
                onReload={() => onReload(row.id)}
              />
            )}
          </li>
        ))}
        {!rows.length && (
          <li className="px-2.5 py-6 text-center text-sm text-content-muted">
            {t('custom-ui:studio.noMatches')}
          </li>
        )}
      </ul>
      {footer}
      <p
        data-studio-hot-reload
        className="flex flex-none flex-col gap-0.5 border-t border-stroke px-3.5 py-2.5 text-xs text-content-muted"
      >
        <span className="flex items-center gap-1.5 text-content-secondary">
          <span aria-hidden className="size-1.5 rounded-full bg-content-muted" />
          {t('custom-ui:studio.hotReloadOn')}
        </span>
        <span className="break-all">{t('custom-ui:studio.directory', { path: root })}</span>
      </p>
    </section>
  )
}
