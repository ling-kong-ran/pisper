// “更多”启动台：全页磁贴，分组与去向来自 more-navigation，桌面与手机共用（手机三列）。
// 页头搜索框第 1 期按名称、描述与分组筛选磁贴；“我的组件”列出放在更多页面位置的已安装组件。
import { useId, useMemo, useState } from 'react'
import { Search, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  countMoreTiles,
  filterMoreNavigation,
  getMoreNavigation,
  MORE_SEARCH_INPUT_ID,
  type MoreDestination,
  type MoreTile,
} from '@/app/more-navigation'
import { Button } from '@/components/ui/button'
import { useCustomUiComponents } from '@/features/custom-ui/catalog'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useClientStore } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type MorePageProps = {
  onOpen: (destination: MoreDestination) => void
}

export function MorePage({ onOpen }: MorePageProps) {
  const { t, language } = useI18n()
  const headingId = useId()
  const [query, setQuery] = useState('')
  const paletteShortcut = useShortcutLabel('commandPalette')
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  const catalog = useCustomUiComponents()
  // 内置组件（如 Pisper Island）是浮窗，不占“更多页面”位置。
  const components = useMemo(
    () =>
      (catalog.data?.components || [])
        .filter((component) => !component.builtIn)
        .map(({ id, name, description }) => ({ id, name, description })),
    [catalog.data],
  )
  const groups = useMemo(
    () => getMoreNavigation(t, { capabilities, mobileApp, components }),
    [capabilities, components, mobileApp, t],
  )
  const visible = useMemo(
    () => filterMoreNavigation(groups, query, language),
    [groups, language, query],
  )
  const counts = countMoreTiles(groups)

  return (
    <div className="h-full min-h-0 w-full overflow-y-auto bg-background">
      <div className="mx-auto flex w-full max-w-[960px] flex-col gap-6 px-6 pt-6 pb-10 max-[650px]:gap-5 max-[650px]:px-4 max-[650px]:pt-4">
        <header className="flex flex-wrap items-center gap-x-4 gap-y-3">
          <div className="flex min-w-0 items-baseline gap-3">
            <h1
              id={headingId}
              className="text-xl font-semibold tracking-[-.01em] text-content max-[650px]:text-2xl"
            >
              {t('navigation:more.title')}
            </h1>
            <p className="truncate text-sm text-content-muted tabular-nums">
              {t('navigation:more.summary', counts)}
            </p>
          </div>
          <label className="ml-auto flex h-8 w-[min(320px,100%)] items-center gap-2 rounded-sm border border-control bg-panel px-2.5 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring max-[650px]:h-11 max-[650px]:w-full max-[650px]:rounded-[10px]">
            <Search className="size-4 flex-none" aria-hidden="true" />
            <span className="sr-only">{t('navigation:more.search')}</span>
            <input
              id={MORE_SEARCH_INPUT_ID}
              type="search"
              value={query}
              placeholder={t('navigation:more.search')}
              className="w-full min-w-0 border-0 bg-transparent text-[length:var(--app-font-size)] text-content caret-focus outline-none max-[650px]:text-[16px] [&::-webkit-search-cancel-button]:hidden"
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Escape' && query) {
                  event.preventDefault()
                  event.stopPropagation()
                  setQuery('')
                }
              }}
            />
            {query && (
              <button
                type="button"
                className="grid size-6 flex-none place-items-center rounded-xs text-content-muted hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:size-11"
                aria-label={t('navigation:more.clearSearch')}
                onClick={() => setQuery('')}
              >
                <X className="size-3.5" aria-hidden="true" />
              </button>
            )}
          </label>
        </header>
        {visible.length === 0 ? (
          <div role="status" className="flex flex-col items-center gap-2 py-16 text-center">
            <p className="text-sm font-medium text-content">{t('navigation:more.noResults')}</p>
            <p className="max-w-sm text-xs text-content-muted">
              {t('navigation:more.noResultsHint', { shortcut: paletteShortcut || '⌘K' })}
            </p>
            <Button variant="secondary" className="mt-2" onClick={() => setQuery('')}>
              {t('navigation:more.clearSearch')}
            </Button>
          </div>
        ) : (
          visible.map((group) => {
            const groupHeadingId = `${headingId}-${group.id}`
            return (
              <section
                key={group.id}
                aria-labelledby={groupHeadingId}
                className="flex flex-col gap-2.5"
              >
                <h2
                  id={groupHeadingId}
                  className="flex items-center gap-3 text-xs font-semibold tracking-[.02em] text-content-secondary after:h-px after:flex-1 after:bg-stroke-soft"
                >
                  {group.label}
                </h2>
                <ul className="grid grid-cols-3 gap-3 max-[650px]:gap-2">
                  {group.tiles.map((tile) => (
                    <li key={tile.key} className="min-w-0">
                      <MoreTileButton tile={tile} onOpen={onOpen} />
                    </li>
                  ))}
                </ul>
                {group.id === 'components' && !query && catalog.isPending && (
                  <p role="status" className="text-xs text-content-muted">
                    {t('navigation:more.componentsLoading')}
                  </p>
                )}
                {group.id === 'components' && !query && catalog.isError && (
                  <p role="alert" className="flex items-center gap-2 text-xs text-content-muted">
                    {t('navigation:more.componentsError')}
                    <Button variant="ghost" size="xs" onClick={() => void catalog.refetch()}>
                      {t('navigation:more.retry')}
                    </Button>
                  </p>
                )}
              </section>
            )
          })
        )}
      </div>
    </div>
  )
}

function MoreTileButton({
  tile,
  onOpen,
}: {
  tile: MoreTile
  onOpen: (destination: MoreDestination) => void
}) {
  const { t } = useI18n()
  const Icon = tile.icon
  const disabled = Boolean(tile.disabledReason)
  const add = tile.destination.type === 'page' && tile.destination.id === 'components'
  return (
    <button
      type="button"
      data-more-tile={tile.key}
      // 禁用磁贴保持可聚焦，读屏和键盘用户也能读到“仅桌面”原因。
      aria-disabled={disabled || undefined}
      aria-label={
        disabled
          ? t('navigation:more.disabledTile', { label: tile.label, reason: tile.disabledReason })
          : undefined
      }
      className={cn(
        'flex h-full min-h-[116px] w-full min-w-0 flex-col items-start gap-2 rounded-card border bg-solid p-3.5 text-left shadow-card transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:min-h-[132px] max-[650px]:p-3',
        add
          ? 'border-dashed border-stroke-strong bg-transparent shadow-none hover:bg-surface-hover'
          : 'border-stroke hover:border-stroke-strong',
        disabled && 'cursor-not-allowed bg-surface-subtle shadow-none hover:border-stroke',
      )}
      onClick={() => {
        if (!disabled) onOpen(tile.destination)
      }}
    >
      <span
        className={cn(
          'grid size-9 flex-none place-items-center rounded-md bg-surface-muted text-content-secondary ring-1 ring-stroke-soft ring-inset',
          disabled && 'text-content-muted',
        )}
      >
        <Icon className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
      </span>
      <span
        className={cn(
          'max-w-full truncate text-sm font-semibold text-content',
          disabled && 'text-content-secondary',
        )}
      >
        {tile.label}
      </span>
      {/* 三列磁贴很窄：均衡折行，避免最后一行只剩一个字。 */}
      <span className="line-clamp-2 text-xs leading-snug text-balance text-content-muted">
        {tile.disabledReason || tile.description}
      </span>
    </button>
  )
}
