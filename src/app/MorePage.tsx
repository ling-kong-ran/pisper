// “功能”索引（原“更多”，路径 /more 不变；规格 2.7）：居中 760 栏，分组标题 + 两列卡片行
// （图标、名称、一句说明、角标），手机是一列 56 行（规格 7.4）。分组与去向来自 more-navigation，桌面与手机共用。
// 页头搜索框与 ⌘K 同源：按名称、说明与分组筛选卡片（与面板的“功能”分组同一份页面清单），
// 同时列出命中的设置项（设置搜索索引），并可带着关键词转到命令面板继续搜会话、标签与命令。
// 卡片角标（工作流运行中、定时任务启用、渠道断开、MCP 在线）见 useMoreTileBadges。
// “我的组件”列出以整页打开的已安装组件，最后一行是“+ 放置组件”（去组件工作室）。
import { useEffect, useId, useLayoutEffect, useMemo, useState, type KeyboardEvent } from 'react'
import { ChevronRight, Command, Search, X } from 'lucide-react'
import { APP_NAME } from '@/app/brand'
import { useI18n } from '@/app/use-i18n'
import {
  ADD_COMPONENT_KEY,
  filterMoreNavigation,
  getMoreNavigation,
  MORE_SEARCH_INPUT_ID,
  type MoreDestination,
  type MoreTile,
} from '@/app/more-navigation'
import { useMoreTileBadges, type MoreTileBadge } from '@/app/useMoreTileBadges'
import { Button } from '@/components/ui/button'
import { Kbd } from '@/components/ui/kbd'
import { requestCommandPalette } from '@/features/chat/events'
import {
  requestConfigCardHighlight,
  searchConfig,
  type ConfigSearchMatch,
} from '@/features/config/config-search'
import { useFeaturePageComponents } from '@/features/custom-ui/placements'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { useClientStore } from '@/stores/client-store'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'

type MorePageProps = {
  onOpen: (destination: MoreDestination) => void
}

const SETTINGS_RESULT_LIMIT = 6
const TILE_SELECTOR = '[data-more-tile]'

// 离开功能索引时的来源卡片：从卡片进插件页等，再点“‹ 功能”或后退回来时，焦点回到这张卡片（规格 2.3）。
// 只保存在内存里，与全部代理的 board-return 一样，刷新后不恢复。
let returnTileKey = ''

function tileByKey(root: ParentNode | null, key: string) {
  return root?.querySelector<HTMLElement>(`[data-more-tile="${CSS.escape(key)}"]`) ?? null
}

export function MorePage({ onOpen }: MorePageProps) {
  const { t, language } = useI18n()
  const headingId = useId()
  const [query, setQuery] = useState('')
  const paletteShortcut = useShortcutLabel('commandPalette')
  const searchShortcut = useShortcutLabel('focusSearch')
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const mobileApp = useClientStore((state) => state.client === 'mobile-app')
  // 手机外壳：没有 52 顶栏拖拽区，卡片排成一列 56 行；标题与底部标签栏一致，A4 换抽屉前仍叫“更多”。
  const phone = usePhoneShell()
  const title = t('navigation:more.title')
  // “我的组件”= 放到“功能页”的原生组件实例（只要可渲染的：实例已启用、组件原生 ready 且已启用）。
  const featureComponents = useFeaturePageComponents()
  const components = useMemo(
    () =>
      featureComponents.items.map(({ componentId, name, description }) => ({
        id: componentId,
        name,
        description,
      })),
    [featureComponents.items],
  )
  const groups = useMemo(
    () => getMoreNavigation(t, { capabilities, mobileApp, components }),
    [capabilities, components, mobileApp, t],
  )
  const visible = useMemo(
    () => filterMoreNavigation(groups, query, language),
    [groups, language, query],
  )
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
  useEffect(() => {
    // 与全部代理一样写文档标题；离开时还原，下一页自己再写。
    document.title = `${title} · ${APP_NAME}`
    return () => {
      document.title = APP_NAME
    }
  }, [title])
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
  const groupHeading = 'mb-2 text-[13px] font-medium text-content-muted'

  // 卸载时焦点在某张卡片上（键盘 ↵、点击后跳走）就记下它；回来挂载时把焦点还给它。
  // 卡片可能晚一点才画出来（“我的组件”等组件目录），逐帧找，最多 20 帧。
  useLayoutEffect(() => {
    // 不挂 ref：页面根节点的写法由整页模板守卫按源码钉着；卸载清理时这个节点还在文档里。
    const root = document.querySelector<HTMLElement>('[data-features-index]')
    const key = returnTileKey
    returnTileKey = ''
    let frame = 0
    let handle = 0
    let restored = !key
    const attempt = () => {
      const tile = tileByKey(root, key)
      if (tile) {
        tile.focus()
        restored = true
      } else if ((frame += 1) < 20) handle = requestAnimationFrame(attempt)
      else restored = true
    }
    if (key) handle = requestAnimationFrame(attempt)
    return () => {
      cancelAnimationFrame(handle)
      // 还没来得及还焦点就卸载（开发模式 StrictMode 的二次挂载）：把来源卡片留给下一次挂载。
      if (!restored) returnTileKey = key
      const active = document.activeElement
      const tile =
        active instanceof HTMLElement && root?.contains(active)
          ? active.closest<HTMLElement>(TILE_SELECTOR)
          : null
      if (tile?.dataset.moreTile) returnTileKey = tile.dataset.moreTile
    }
  }, [])

  return (
    <div data-features-index className="flex h-full min-h-0 w-full flex-col bg-background">
      {/* 主区顶栏 52：没有控件，只作 Windows 自绘标题栏的拖拽区（右侧 144px 留给窗口按钮），与全部代理一致。 */}
      {!phone && <div data-window-drag-region aria-hidden="true" className="h-[52px] flex-none" />}
      <div
        data-features-scroller
        className="min-h-0 flex-1 overflow-y-auto overscroll-contain"
        onKeyDown={moveTileFocus}
      >
        <div
          data-features-column
          className="mx-auto flex w-full max-w-[808px] flex-col px-6 pb-16 max-[650px]:px-4 max-[650px]:pt-4 max-[650px]:pb-8"
        >
          <header data-focus-region="header" className="flex flex-col">
            {/* 手机：页名在外壳顶栏里，这里的 h1 只给读屏（规格 7.1），说明也省掉。 */}
            <h1
              id={headingId}
              className={cn(
                'text-2xl leading-8 font-semibold tracking-[-.01em] text-content',
                phone && 'sr-only',
              )}
            >
              {title}
            </h1>
            <p className={cn('mt-1 text-sm text-content-muted', phone && 'hidden')}>
              {/* 手机没有实体键盘：不写快捷键，只说命令面板。 */}
              {paletteShortcut && !phone
                ? t('navigation:more.lede', { shortcut: paletteShortcut })
                : t('navigation:more.ledeNoShortcut')}
            </p>
            <label
              className={cn(
                'relative mt-5 flex h-9 w-full items-center gap-2 rounded-[var(--r-md)] border border-control bg-panel px-3 text-content-muted focus-within:border-focus focus-within:ring-3 focus-within:ring-focus-ring max-[650px]:h-11',
                phone && 'mt-0',
              )}
            >
              <Search className="size-4 flex-none" aria-hidden="true" />
              <span className="sr-only">{t('navigation:more.search')}</span>
              <input
                id={MORE_SEARCH_INPUT_ID}
                type="search"
                value={query}
                placeholder={t('navigation:more.search')}
                aria-keyshortcuts={searchShortcut || undefined}
                className="w-full min-w-0 border-0 bg-transparent text-[length:var(--app-font-size)] text-content caret-focus outline-none max-[650px]:text-[16px] [&::-webkit-search-cancel-button]:hidden"
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === 'Escape' && query) {
                    event.preventDefault()
                    event.stopPropagation()
                    setQuery('')
                  } else if (event.key === 'ArrowDown') {
                    // ↓ 从搜索框进入第一张卡片（筛选后即第一个结果），↵ 打开。
                    const first = event.currentTarget
                      .closest('[data-features-scroller]')
                      ?.querySelector<HTMLElement>(TILE_SELECTOR)
                    if (first) {
                      event.preventDefault()
                      event.stopPropagation()
                      first.focus()
                    }
                  }
                }}
              />
              {query ? (
                <button
                  type="button"
                  className="grid size-6 flex-none place-items-center rounded-xs text-content-muted hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:size-11"
                  aria-label={t('navigation:more.clearSearch')}
                  onClick={() => setQuery('')}
                >
                  <X className="size-3.5" aria-hidden="true" />
                </button>
              ) : (
                searchShortcut && (
                  <Kbd className="flex-none pointer-coarse:hidden">{searchShortcut}</Kbd>
                )
              )}
            </label>
          </header>
          {visible.length === 0 && settingsMatches.length === 0 ? (
            <div
              role="status"
              data-features-empty
              className="flex flex-col items-center gap-2 py-16 text-center"
            >
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
                  data-more-group={group.id}
                  aria-labelledby={groupHeadingId}
                  className="mt-7 flex flex-col"
                >
                  <h2 id={groupHeadingId} className={groupHeading}>
                    {group.label}
                  </h2>
                  <ul className="grid grid-cols-2 gap-2 max-[650px]:grid-cols-1">
                    {group.tiles.map((tile) => (
                      <li key={tile.key} className="min-w-0">
                        <MoreTileButton tile={tile} badge={badges.get(tile.key)} onOpen={onOpen} />
                      </li>
                    ))}
                  </ul>
                  {group.id === 'components' && !query && featureComponents.isLoading && (
                    <p role="status" className="mt-2 text-xs text-content-muted">
                      {t('navigation:more.componentsLoading')}
                    </p>
                  )}
                  {group.id === 'components' && !query && Boolean(featureComponents.error) && (
                    <p
                      role="alert"
                      className="mt-2 flex items-center gap-2 text-xs text-content-muted"
                    >
                      {t('navigation:more.componentsError')}
                      <Button variant="ghost" size="xs" onClick={featureComponents.refetch}>
                        {t('navigation:more.retry')}
                      </Button>
                    </p>
                  )}
                </section>
              )
            })
          )}
          {query.trim() && (visible.length > 0 || settingsMatches.length > 0) && (
            <section aria-labelledby={`${headingId}-search`} className="mt-7 flex flex-col">
              <h2 id={`${headingId}-search`} className={groupHeading}>
                {t('navigation:more.settingsResults')}
              </h2>
              {settingsMatches.length > 0 && (
                <ul
                  data-more-settings-results
                  className="mb-2 flex flex-col divide-y divide-stroke-soft overflow-hidden rounded-card border border-stroke"
                >
                  {settingsMatches.map((match) => (
                    <li key={match.entry.id}>
                      <button
                        type="button"
                        className="flex min-h-11 w-full min-w-0 items-center gap-3 px-3.5 py-2 text-left transition-colors duration-(--d-fast) hover:bg-surface-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
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
    </div>
  )
}

// 卡片之间的方向键：←/→ 按阅读顺序前后移动，↑/↓ 移到上 / 下一行里横向最近的那张（跨分组也连续），
// Home / End 到第一 / 最后一张；第一行再按 ↑ 回到搜索框。只移动焦点，打开仍是 ↵ / 空格（按钮原生行为）。
function moveTileFocus(event: KeyboardEvent<HTMLElement>) {
  const current = event.target
  if (!(current instanceof HTMLElement) || !current.matches(TILE_SELECTOR)) return
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return
  const tiles = [...event.currentTarget.querySelectorAll<HTMLElement>(TILE_SELECTOR)]
  const index = tiles.indexOf(current)
  if (index < 0) return
  let next: HTMLElement | undefined
  if (event.key === 'ArrowRight') next = tiles[index + 1]
  else if (event.key === 'ArrowLeft') next = tiles[index - 1]
  else if (event.key === 'Home') next = tiles[0]
  else if (event.key === 'End') next = tiles.at(-1)
  else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    const from = current.getBoundingClientRect()
    const down = event.key === 'ArrowDown'
    const center = from.left + from.width / 2
    // 先找相邻的那一行（顶边最近），再在这一行里取横向中心最近的卡片。
    const rows = tiles
      .map((tile) => ({ tile, box: tile.getBoundingClientRect() }))
      .filter(({ box }) => (down ? box.top >= from.bottom - 1 : box.bottom <= from.top + 1))
    if (rows.length > 0) {
      const edge = down
        ? Math.min(...rows.map(({ box }) => box.top))
        : Math.max(...rows.map(({ box }) => box.top))
      next = rows
        .filter(({ box }) => Math.abs(box.top - edge) < 2)
        .sort(
          (a, b) =>
            Math.abs(a.box.left + a.box.width / 2 - center) -
            Math.abs(b.box.left + b.box.width / 2 - center),
        )[0]?.tile
    } else if (!down) {
      document.getElementById(MORE_SEARCH_INPUT_ID)?.focus()
      event.preventDefault()
      return
    }
  } else return
  event.preventDefault()
  next?.focus()
}

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
  // 只有“我的组件”末行的“+ 放置组件”是虚线空位；“扩展 › 组件工作室”也去组件页，但它是普通入口。
  const add = tile.key === ADD_COMPONENT_KEY
  return (
    <button
      type="button"
      data-more-tile={tile.key}
      // 禁用卡片保持可聚焦，读屏和键盘用户也能读到“仅桌面端可用”的原因。
      aria-disabled={disabled || undefined}
      aria-label={
        disabled
          ? t('navigation:more.disabledTile', { label: tile.label, reason: tile.disabledReason })
          : undefined
      }
      className={cn(
        'flex min-h-16 w-full min-w-0 items-center gap-3 rounded-card border px-3.5 py-2.5 text-left transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus max-[650px]:min-h-14 max-[650px]:py-2',
        add ? 'border-dashed border-stroke-strong' : 'border-stroke',
        disabled ? 'cursor-not-allowed' : 'hover:bg-surface-hover',
      )}
      onClick={() => {
        if (disabled) return
        // Safari 点击按钮不给它焦点：卸载时看不到焦点卡片，这里先记下来源。
        returnTileKey = tile.key
        onOpen(tile.destination)
      }}
    >
      <span
        aria-hidden="true"
        className={cn(
          'grid size-[34px] flex-none place-items-center rounded-[var(--r-md)] max-[650px]:size-8',
          add ? 'text-content-muted' : 'bg-surface-subtle text-content-secondary',
          disabled && 'text-content-muted',
        )}
      >
        <Icon className="size-[18px]" strokeWidth={1.75} />
      </span>
      <span className="flex min-w-0 flex-1 flex-col">
        <span
          className={cn(
            'truncate text-sm font-medium text-content',
            (disabled || add) && 'text-content-secondary',
          )}
        >
          {tile.label}
        </span>
        {/* 一句说明一行放下（两列卡片约 16 个汉字宽）；组件自带的长说明截断，全文仍在按钮文字里供读屏读出。 */}
        <span className="truncate text-xs text-content-muted">
          {tile.disabledReason || tile.description}
        </span>
      </span>
      {badge && !disabled && (
        <span
          data-more-badge={badge.tone}
          className={cn(
            'flex max-w-[45%] flex-none items-center gap-1.5 text-xs whitespace-nowrap tabular-nums',
            // 琥珀只表示需要你（规格 6.1）：渠道断开是唯一要人处理的角标；运行中、启用、在线只是计数。
            badge.tone === 'attn' ? 'font-medium text-st-attn' : 'text-content-muted',
          )}
        >
          {badge.tone === 'attn' && (
            <span aria-hidden="true" className="size-2 flex-none rounded-full bg-st-attn-fill" />
          )}
          <span className="truncate">{badge.label}</span>
        </span>
      )}
    </button>
  )
}
