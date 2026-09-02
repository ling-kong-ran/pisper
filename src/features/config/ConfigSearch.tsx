// 设置搜索框：位于设置页 PageHeader 区域（shadcn Input + Search 图标，Esc 清空）。
// 输入时按跨语言索引搜索设置分区与卡片，结果以下拉列表展示；
// 点击结果跳转到对应分区并高亮目标卡片（见 config-search.ts）。
import { useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { Search } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Input } from '@/components/ui/input'
import { cn } from '@/lib/utils'
import { requestConfigCardHighlight, searchConfig, type ConfigSearchMatch } from './config-search'

type ConfigSearchBoxProps = {
  query: string
  onQueryChange: (query: string) => void
  // 选中结果后跳转目标分区（高亮请求由组件内部发起）
  onSelect: (section: string) => void
  inputRef: RefObject<HTMLInputElement | null>
}

export function ConfigSearchBox({
  query,
  onQueryChange,
  onSelect,
  inputRef,
}: ConfigSearchBoxProps) {
  const { t, language } = useI18n()
  const [open, setOpen] = useState(false)
  const [activeIndex, setActiveIndex] = useState(0)
  const containerRef = useRef<HTMLDivElement>(null)
  const results = useMemo(() => searchConfig(query, language), [language, query])

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
    // 先登记高亮请求再切分区：跨分区时由 ConfigPage 在新区内容挂载后消费，
    // 同分区时由事件监听即时触发。
    requestConfigCardHighlight(match.entry.card, match.entry.section)
    onSelect(match.entry.section)
  }

  const showDropdown = open && query.trim().length > 0

  return (
    <div
      ref={containerRef}
      className="relative w-[min(250px,24vw)] max-[900px]:w-[190px] max-[650px]:col-start-1 max-[650px]:row-start-1 max-[650px]:w-full max-[650px]:min-w-0"
    >
      <Search
        size={15}
        className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-[var(--text-muted)]"
      />
      <Input
        ref={inputRef}
        value={query}
        title={t('navigation:pageHeader.search')}
        aria-label={t('navigation:pageHeader.search')}
        className="h-[34px] pl-8 text-[13px] in-data-[density=compact]:h-[30px]"
        placeholder={t('config:configSearch.placeholder')}
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
      {showDropdown && (
        <div className="absolute top-full left-0 z-50 mt-1.5 w-[max(100%,280px)] overflow-hidden rounded-[var(--r-md)] border border-[var(--stroke)] bg-[var(--solid)] shadow-[var(--sh-floating)]">
          {results.length === 0 ? (
            <p className="px-3 py-3 text-[12px] text-[var(--text-muted)]">
              {t('config:configSearch.noResults')}
            </p>
          ) : (
            <ul className="max-h-[280px] overflow-auto py-1">
              {results.map((match, index) => (
                <li key={match.entry.id}>
                  <button
                    type="button"
                    className={cn(
                      'flex w-full flex-col gap-0.5 px-3 py-2 text-left',
                      index === activeIndex
                        ? 'bg-[var(--accent-soft)]'
                        : 'hover:bg-[var(--surface-subtle)]',
                    )}
                    onMouseEnter={() => setActiveIndex(index)}
                    onClick={() => select(match)}
                  >
                    <span className="text-[13px] text-[var(--text)]">{match.title}</span>
                    <span className="text-[11px] text-[var(--text-muted)]">
                      {match.sectionTitle}
                    </span>
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
