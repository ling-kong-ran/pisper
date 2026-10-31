// 设置搜索框：位于设置页 PageHeader 区域（shadcn Input + Search 图标，Esc 清空）。
// 输入时按跨语言索引搜索设置分区与卡片，结果以下拉列表展示；
// 点击结果跳转到对应分区并高亮目标卡片（见 config-search.ts）。
import { useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { Search } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Input } from '@/components/ui/input'
import { Kbd } from '@/components/ui/kbd'
import { useShortcutLabel } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { requestConfigCardHighlight, searchConfig, type ConfigSearchMatch } from './config-search'

type ConfigSearchBoxProps = {
  query: string
  onQueryChange: (query: string) => void
  // 选中结果后跳转目标分区（高亮请求由组件内部发起）
  onSelect: (section: string, view?: 'appearance' | 'widgets') => void
  inputRef: RefObject<HTMLInputElement | null>
  placement?: 'toolbar' | 'sidebar'
}

export function ConfigSearchBox({
  query,
  onQueryChange,
  onSelect,
  inputRef,
  placement = 'toolbar',
}: ConfigSearchBoxProps) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const containerRef = useRef<HTMLDivElement>(null)
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const shortcut = useShortcutLabel('focusSearch')
  const results = useMemo(
    () =>
      searchConfig(query, language, {
        capabilities,
        mobileApp: Boolean(window.__PISPER_MOBILE_APP__),
      }),
    [language, query, capabilities],
  )

  // 查询变化时重置键盘选中项，避免高亮停在越界位置。
  useEffect(() => setActiveIndex(0), [query])

  // 点击搜索框以外区域收起下拉。
  useEffect(() => {
    if (!open) return undefined
    const onPointerDown = (event: PointerEvent) => {
      if (containerRef.current && !containerRef.current.contains(event.target as Node))
        setOpen(false)
    }
    document.addEventListener('pointerdown', onPointerDown)
    return () => document.removeEventListener('pointerdown', onPointerDown)
  }, [open])

  const select = (match: ConfigSearchMatch) => {
    setOpen(false)
    onQueryChange('')
    // 独立组件已迁到组件页：不登记卡片高亮（设置里已没有这张卡），由应用壳直接去 /components。
    if (match.entry.card === 'interface-custom-ui') {
      onSelect(match.entry.section, 'widgets')
      return
    }
    // 先登记高亮请求再切分区：跨分区时由 ConfigPage 在新区内容挂载后消费，
    // 同分区时由事件监听即时触发。
    requestConfigCardHighlight(match.entry.card, match.entry.section, {
      targetId: match.entry.targetId,
      targetText: match.title,
      expandGroup: match.entry.expandGroup,
    })
    onSelect(match.entry.section)
  }

  const showDropdown = open && query.trim().length > 0

  return (
    <div
      ref={containerRef}
      className={cn(
        'relative',
        placement === 'sidebar'
          ? 'w-full'
          : 'w-[min(250px,24vw)] max-[900px]:w-[190px] max-[650px]:col-start-1 max-[650px]:row-start-1 max-[650px]:w-full max-[650px]:min-w-0',
      )}
    >
      <Search
        size={15}
        aria-hidden="true"
        className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-content-muted"
      />
      <Input
        ref={inputRef}
        value={query}
        title={t('navigation:pageHeader.search')}
        aria-label={t('navigation:pageHeader.search')}
        className={cn(
          'h-8.5 pl-8 text-sm pointer-coarse:min-h-11',
          // 侧栏搜索是一枚平键：键帽面 + 键帽描边，右侧留出快捷键小键帽的位置。
          // 分区导航较窄：占位文字放不下时以省略号收尾，不在词中间硬裁。
          placement === 'sidebar' && 'h-8 border-key-border bg-key pr-9 text-ellipsis shadow-key',
        )}
        placeholder={
          // 侧栏只有 268 宽：长占位会被截成半个英文词，换短的。
          placement === 'sidebar'
            ? t('config:configSearch.placeholderShort')
            : t('config:configSearch.placeholder')
        }
        onChange={(event) => {
          onQueryChange(event.target.value)
          setOpen(true)
        }}
        onFocus={() => {
          if (query.trim()) setOpen(true)
        }}
        onKeyDown={(event) => {
          if (event.key === 'Escape') {
            // Esc 清空搜索词并收起下拉；阻止冒泡避免触发壳层的全局 Esc 逻辑。
            event.stopPropagation()
            onQueryChange('')
            setOpen(false)
          } else if (event.key === 'ArrowDown') {
            event.preventDefault()
            setActiveIndex((index) => Math.min(index + 1, results.length - 1))
          } else if (event.key === 'ArrowUp') {
            event.preventDefault()
            setActiveIndex((index) => Math.max(index - 1, 0))
          } else if (event.key === 'Enter' && results[activeIndex]) {
            event.preventDefault()
            select(results[activeIndex])
          }
        }}
      />
      {placement === 'sidebar' && shortcut && !query && (
        <Kbd className="absolute top-1/2 right-2 -translate-y-1/2 pointer-coarse:hidden">
          {shortcut}
        </Kbd>
      )}
      {showDropdown && (
        <div className="absolute top-full left-0 z-50 mt-1.5 w-full overflow-hidden rounded-md bg-solid shadow-float">
          {results.length === 0 ? (
            <p role="status" className="px-3 py-3 text-xs text-content-muted">
              {t('config:configSearch.noResults')}
            </p>
          ) : (
            <ul className="max-h-[280px] overflow-auto py-1">
              {results.map((match, index) => (
                <li key={match.entry.id}>
                  <button
                    type="button"
                    className={cn(
                      'flex w-full flex-col gap-0.5 px-3 py-1.5 text-left pointer-coarse:min-h-11',
                      index === activeIndex ? 'bg-surface-hover' : 'hover:bg-surface-subtle',
                    )}
                    onMouseEnter={() => setActiveIndex(index)}
                    onClick={() => select(match)}
                  >
                    <span className="text-sm text-content">{match.title}</span>
                    <span className="text-xs text-content-muted">{match.sectionTitle}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </div>
  )
}
