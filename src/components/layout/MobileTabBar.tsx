// 手机底部标签栏：与 ActivityRail 共用分组数据。点未选中的标签回到该组上次停留的页面，
// 再点已选中的标签才展开组内页面，避免每次切组都多一步菜单。
import { useEffect, useRef, useState } from 'react'
import type { LucideIcon } from 'lucide-react'
import { Check, ChevronUp } from 'lucide-react'
import type { ActivityGroup } from '@/components/layout/ActivityRail'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { cn } from '@/lib/utils'

type MobileTab = {
  id: string
  label: string
  icon: LucideIcon
  active: boolean
  group?: ActivityGroup
}

export function MobileTabBar({
  groups,
  activeId,
  onSelect,
  shortLabels = {},
  settings,
  label,
  groupPagesLabel,
}: {
  groups: ActivityGroup[]
  activeId: string
  onSelect: (id: string) => void
  // 侧栏分组名偏长（如“工作台与项目”），底栏按组 id 换成短名。
  shortLabels?: Record<string, string>
  // 插件、星忆等页同时属于某个分组和设置；归属由壳层按进入来源判定后传入，
  // 归设置时 activeId 为空，分组不再点亮，保证底栏与设置分区条一致。
  settings: {
    label: string
    icon: LucideIcon
    active: boolean
    onSelect: () => void
  }
  label: string
  groupPagesLabel: (group: string) => string
}) {
  const lastVisitedRef = useRef<Record<string, string>>({})
  const [sheetGroupId, setSheetGroupId] = useState<string | null>(null)
  const activeTabId = settings.active
    ? 'settings'
    : groups.find((group) => group.items.some((item) => item.id === activeId))?.id || null

  useEffect(() => {
    if (activeTabId && activeTabId !== 'settings') lastVisitedRef.current[activeTabId] = activeId
  }, [activeTabId, activeId])

  const tabs: MobileTab[] = [
    ...groups.map((group) => ({
      id: group.id,
      label: shortLabels[group.id] || group.label,
      icon: group.icon,
      active: group.id === activeTabId,
      group,
    })),
    {
      id: 'settings',
      label: settings.label,
      icon: settings.icon,
      active: activeTabId === 'settings',
    },
  ]
  const sheetGroup = groups.find((group) => group.id === sheetGroupId)

  const selectTab = (tab: MobileTab) => {
    if (!tab.group) {
      if (!tab.active) settings.onSelect()
      return
    }
    const enabled = tab.group.items.filter((item) => !item.disabled)
    if (tab.active) {
      if (enabled.length > 1) setSheetGroupId(tab.group.id)
      return
    }
    const remembered = lastVisitedRef.current[tab.group.id]
    const target = enabled.find((item) => item.id === remembered) || enabled[0]
    if (!target) return
    onSelect(target.id)
  }

  return (
    <>
      <nav
        aria-label={label}
        data-testid="mobile-tab-bar"
        className="mobile-tab-bar grid flex-none grid-flow-col auto-cols-fr border-t border-stroke bg-panel pb-[var(--pisper-safe-area-bottom)] [[data-mobile-keyboard='open']_&]:hidden"
      >
        {tabs.map((tab) => {
          const expandable =
            tab.active && (tab.group?.items.filter((item) => !item.disabled).length || 0) > 1
          return (
            <button
              key={tab.id}
              type="button"
              aria-current={tab.active ? 'page' : undefined}
              aria-haspopup={expandable ? 'dialog' : undefined}
              aria-label={expandable ? groupPagesLabel(tab.label) : undefined}
              // 选中标签只用强调色文字/图标 + 顶边 28×2 刻度表达，不加底色块；聚焦用内嵌焦点环与之区分。
              className={cn(
                'relative flex h-14 min-w-0 flex-col items-center justify-center gap-1 border-0 bg-transparent px-1 text-xs leading-none text-content-muted transition-colors duration-[var(--d-fast)] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus focus-visible:ring-inset active:bg-surface-subtle',
                tab.active &&
                  'font-medium text-accent-fg before:absolute before:top-[-1px] before:left-1/2 before:h-0.5 before:w-7 before:-translate-x-1/2 before:rounded-b-[2px] before:bg-star',
              )}
              onClick={() => selectTab(tab)}
            >
              <tab.icon
                className="size-[22px]"
                strokeWidth={tab.active ? 2 : 1.75}
                aria-hidden="true"
              />
              <span className="flex max-w-full items-center gap-0.5">
                <span className="truncate">{tab.label}</span>
                {expandable && <ChevronUp className="size-3 shrink-0" aria-hidden="true" />}
              </span>
            </button>
          )
        })}
      </nav>
      <Sheet open={Boolean(sheetGroup)} onOpenChange={(open) => !open && setSheetGroupId(null)}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          className="gap-0 px-2 pt-2 pb-[calc(8px+var(--pisper-safe-area-bottom))]"
        >
          <div className="mx-auto mb-1 h-1 w-9 rounded-full bg-stroke-strong" aria-hidden="true" />
          <SheetHeader className="px-3 pt-2 pb-1">
            <SheetTitle className="text-xs font-medium tracking-[.02em] text-content-muted">
              {sheetGroup?.label}
            </SheetTitle>
          </SheetHeader>
          <div className="flex flex-col">
            {sheetGroup?.items.map((item) => {
              const current = item.id === activeId
              return (
                <button
                  key={item.id}
                  type="button"
                  disabled={item.disabled}
                  aria-current={current ? 'page' : undefined}
                  className={cn(
                    'flex min-h-[52px] items-center gap-3 rounded-md border-0 bg-transparent px-3 text-left text-[15px] text-content active:bg-surface-subtle focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-focus focus-visible:ring-inset disabled:opacity-40',
                    current && 'font-medium',
                  )}
                  onClick={() => {
                    setSheetGroupId(null)
                    if (!current) onSelect(item.id)
                  }}
                >
                  <item.icon
                    className={cn(
                      'size-5 shrink-0',
                      current ? 'text-accent-fg' : 'text-content-muted',
                    )}
                    aria-hidden="true"
                  />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  {current && (
                    <Check className="size-4 shrink-0 text-accent-fg" aria-hidden="true" />
                  )}
                </button>
              )
            })}
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}
