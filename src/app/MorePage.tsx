// “更多”启动台：全页磁贴，分组与去向来自 more-navigation，桌面与手机共用（手机三列）。
// 页头搜索框与 ⌘K 同源：按名称、描述与分组筛选磁贴（与面板的“更多”分组同一份页面清单），
// 同时列出命中的设置项（设置搜索索引），并可带着关键词转到命令面板继续搜会话、标签与命令。
// 磁贴角标（工作流运行中、定时任务启用、渠道断开、MCP 在线）见 useMoreTileBadges。
// “我的组件”列出放在更多页面位置的已安装组件。
import { useId, useMemo, useState } from 'react'
import { ChevronRight, Command, Search, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  countMoreTiles,
  filterMoreNavigation,
  getMoreNavigation,
  MORE_SEARCH_INPUT_ID,
  type MoreDestination,
  type MoreTile,
} from '@/app/more-navigation'
import { useMoreTileBadges, type MoreTileBadge } from '@/app/useMoreTileBadges'
import { Button } from '@/components/ui/button'
import { requestCommandPalette } from '@/features/chat/events'
import {
  requestConfigCardHighlight,
  searchConfig,
  type ConfigSearchMatch,
} from '@/features/config/config-search'
import { useCustomUiComponents } from '@/features/custom-ui/catalog'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { usePhoneShell } from '@/hooks/usePhoneShell'
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
  // 手机外壳的底部标签栏没有“组件”：组件页入口作为“我的组件”的第一格出现在这里。
  const phone = usePhoneShell()
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
    // 规格 v2：组件工作室不再是一级入口（标题栏标签已退役），桌面与手机都从“功能”的组件分组进入；
    // 功能索引在 A3 重排时移到“扩展 › 组件工作室”。
    () => getMoreNavigation(t, { capabilities, mobileApp, components, componentsTile: true }),
    [capabilities, components, mobileApp, t],
  )
  const visible = useMemo(
    () => filterMoreNavigation(groups, query, language),
    [groups, language, query],
  )
  const counts = countMoreTiles(groups)
  const tileKeys = useMemo(
    () => new Set(groups.flatMap((group) => group.tiles.map((tile) => tile.key))),
    [groups],
  )
  const badges = useMoreTileBadges(t, tileKeys)
  // 设置项与 ⌘K 的设置搜索同一份索引；只列前几条，其余在设置页的搜索里看。
  const settingsMatches = useMemo(
    () =>
      query.trim()
        ? searchConfig(query, language, { capabilities, mobileApp }).slice(0, SETTINGS_RESULT_LIMIT)
        : [],
    [capabilities, language, mobileApp, query],
  )
  const openSetting = (match: ConfigSearchMatch) => {
    // 已迁到组件页的“独立组件”条目直接去组件页；其余先登记卡片高亮，再切到对应设置分区。
    if (match.entry.card === 'interface-custom-ui') {
      onOpen({ type: 'page', id: 'components' })
      return
    }
    requestConfigCardHighlight(match.entry.card, match.entry.section, {
      targetId: match.entry.targetId,
      targetText: match.title,
      expandGroup: match.entry.expandGroup,
    })
    onOpen({ type: 'config', id: match.entry.section })
  }
  const searchPalette = (
    <Button
      variant="outline"
      className="h-auto min-h-11 w-full justify-start gap-2.5 py-2 text-left whitespace-normal"
      data-more-palette-search
      onClick={() => requestCommandPalette(query.trim())}
    >
      {/* 手机没有 ⌘ 键：触屏上用搜索图标表示“交给命令面板继续搜”。 */}
      {phone ? (
        <Search className="flex-none" aria-hidden="true" />
      ) : (
        <Command className="flex-none" aria-hidden="true" />
      )}
      <span className="flex min-w-0 flex-col">
        <span className="truncate text-sm text-content">
          {t('navigation:more.searchPalette', { query: query.trim() })}
        </span>
        <span className="text-xs font-normal text-content-muted">
          {t('navigation:more.searchPaletteHint')}
        </span>
      </span>
    </Button>
  )

  return (
    <div className="h-full min-h-0 w-full overflow-y-auto bg-background">
      <div className="mx-auto flex w-full max-w-[960px] flex-col gap-6 px-6 pt-6 pb-10 max-[650px]:gap-5 max-[650px]:px-4 max-[650px]:pt-4 [[data-custom-titlebar]_&]:pt-14">
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
        {visible.length === 0 && settingsMatches.length === 0 ? (
          <div role="status" className="flex flex-col items-center gap-2 py-16 text-center">
            <p className="text-sm font-medium text-content">{t('navigation:more.noResults')}</p>
            <p className="max-w-sm text-xs text-content-muted">
              {t('navigation:more.noResultsHint', { shortcut: paletteShortcut || '⌘K' })}
            </p>
            <div className="mt-2 flex w-full max-w-sm flex-col gap-2">
              {searchPalette}
              <Button variant="secondary" className="min-h-11" onClick={() => setQuery('')}>
                {t('navigation:more.clearSearch')}
              </Button>
            </div>
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
                      <MoreTileButton tile={tile} badge={badges.get(tile.key)} onOpen={onOpen} />
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
        {query.trim() && (visible.length > 0 || settingsMatches.length > 0) && (
          <section aria-labelledby={`${headingId}-search`} className="flex flex-col gap-2.5">
            <h2
              id={`${headingId}-search`}
              className="flex items-center gap-3 text-xs font-semibold tracking-[.02em] text-content-secondary after:h-px after:flex-1 after:bg-stroke-soft"
            >
              {t('navigation:more.settingsResults')}
            </h2>
            {settingsMatches.length > 0 && (
              <ul
                data-more-settings-results
                className="flex flex-col divide-y divide-stroke-soft overflow-hidden rounded-card border border-stroke bg-solid"
              >
                {settingsMatches.map((match) => (
                  <li key={match.entry.id}>
                    <button
                      type="button"
                      className="flex min-h-11 w-full min-w-0 items-center gap-3 px-3.5 py-2 text-left transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:-outline-offset-2"
                      onClick={() => openSetting(match)}
                    >
                      <span className="flex min-w-0 flex-1 flex-col">
                        <span className="truncate text-sm text-content">{match.title}</span>
                        {match.sectionTitle !== match.title && (
                          <span className="truncate text-xs text-content-muted">
                            {match.sectionTitle}
                          </span>
                        )}
                      </span>
                      <ChevronRight
                        className="size-4 flex-none text-content-muted"
                        aria-hidden="true"
                      />
                    </button>
                  </li>
                ))}
              </ul>
            )}
            {searchPalette}
          </section>
        )}
      </div>
    </div>
  )
}

// 角标色：运行中用运行青，断开用琥珀（需要人处理），启用 / 在线只是计数，用中性色。
const BADGE_TONES: Record<MoreTileBadge['tone'], { text: string; dot: string }> = {
  run: { text: 'text-st-run', dot: 'bg-st-run-fill' },
  attn: { text: 'text-st-attn', dot: 'bg-st-attn-solid' },
  neutral: { text: 'text-content-secondary', dot: 'bg-content-muted' },
}
const SETTINGS_RESULT_LIMIT = 6

function MoreTileButton({
  tile,
  badge,
  onOpen,
}: {
  tile: MoreTile
  badge?: MoreTileBadge
  onOpen: (destination: MoreDestination) => void
}) {
  const { t } = useI18n()
  const Icon = tile.icon
  const disabled = Boolean(tile.disabledReason)
  // 只有“我的组件”末格的“+ 添加”是虚线占位；手机上导航用的“组件”磁贴（shell:components）也去组件页，
  // 但它是普通入口，不能画成待添加的空位。
  const add = tile.key === 'page:components'
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
        'relative flex h-full min-h-[116px] w-full min-w-0 flex-col items-start gap-2 rounded-card border bg-solid p-3.5 text-left shadow-card transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:min-h-[132px] max-[650px]:p-3',
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
          'grid size-9 flex-none place-items-center rounded-md bg-surface-muted text-content-secondary ring-1 ring-stroke-soft ring-inset max-[650px]:size-8',
          disabled && 'text-content-muted',
        )}
      >
        <Icon className="size-[18px]" strokeWidth={1.75} aria-hidden="true" />
      </span>
      {badge && !disabled && (
        <span
          data-more-badge={badge.tone}
          className={cn(
            // 手机三列磁贴很窄：图标缩到 32px、角标贴近右上角，给“1 运行中”留足宽度。
            'absolute top-3 right-3 flex max-w-[calc(100%-64px)] items-center gap-1 text-xs font-medium whitespace-nowrap tabular-nums max-[650px]:top-2.5 max-[650px]:right-2 max-[650px]:max-w-[calc(100%-50px)]',
            BADGE_TONES[badge.tone].text,
          )}
        >
          <span
            aria-hidden="true"
            className={cn('size-1.5 flex-none rounded-full', BADGE_TONES[badge.tone].dot)}
          />
          <span className="truncate">{badge.label}</span>
        </span>
      )}
      <span
        className={cn(
          'max-w-full truncate text-sm font-semibold text-content',
          disabled && 'text-content-secondary',
        )}
      >
        {tile.label}
      </span>
      {/* 三列磁贴很窄（360px 手机上约 6 个汉字宽）：内置说明按这个宽度写短，中文一行放下、英文两行内按词断开；
          组件自带的长说明均衡折行、截到两行，全文仍在按钮文字里供读屏读出；
          “甲 · 乙”式说明把间隔点粘在前半句后面，折行时不会让第二行以“·”开头。 */}
      <span className="line-clamp-2 text-xs leading-snug text-balance text-content-muted">
        {(tile.disabledReason || tile.description).replaceAll(' · ', '\u00a0· ')}
      </span>
    </button>
  )
}
