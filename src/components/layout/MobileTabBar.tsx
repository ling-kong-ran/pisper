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
  settings: {
    label: string
    icon: LucideIcon
    contains: (id: string) => boolean
    onSelect: () => void
  }
  label: string
  groupPagesLabel: (group: string) => string
}) {
  const lastVisitedRef = useRef<Record<string, string>>({})
  const [sheetGroupId, setSheetGroupId] = useState<string | null>(null)
  // 插件、星忆等页同时属于某个分组和设置；以最后点过的标签为归属，避免从设置进入后高亮跳走。
  const [owner, setOwner] = useState<string | null>(null)
  const owns = (tabId: string, id: string) =>
    tabId === 'settings'
      ? settings.contains(id)
      : Boolean(groups.find((group) => group.id === tabId)?.items.some((item) => item.id === id))
  const ownerValid = owner !== null && owns(owner, activeId)
  const activeTabId = ownerValid
    ? owner
    : groups.find((group) => group.items.some((item) => item.id === activeId))?.id ||
      (settings.contains(activeId) ? 'settings' : null)

  // 点击标签与路由切换不在同一帧；归属等页面真正变化后再落定，离开归属页时才清除。
  const pendingOwnerRef = useRef<string | null>(null)
  const ownsRef = useRef(owns)
  ownsRef.current = owns
  useEffect(() => {
    const pending = pendingOwnerRef.current
    pendingOwnerRef.current = null
    setOwner(
      (current) => pending ?? (current && ownsRef.current(current, activeId) ? current : null),
    )
  }, [activeId])
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
      if (tab.active) setOwner('settings')
      else {
        pendingOwnerRef.current = 'settings'
        settings.onSelect()
      }
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
    pendingOwnerRef.current = tab.group.id
    onSelect(target.id)
  }

  return (
    <>
      <nav
        aria-label={label}
        data-testid="mobile-tab-bar"
        className="mobile-tab-bar grid flex-none grid-flow-col auto-cols-fr border-t border-[var(--stroke-soft)] bg-[var(--app-chrome)] pb-[var(--pisper-safe-area-bottom)] [[data-mobile-keyboard='open']_&]:hidden"
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
              className={cn(
                'flex h-14 min-w-0 flex-col items-center justify-center gap-0.5 border-0 bg-transparent px-1 text-[11px] font-medium text-muted-foreground transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--focus)]',
                tab.active && 'text-content',
              )}
              onClick={() => selectTab(tab)}
            >
              <span
                className={cn(
                  'grid h-7 w-12 place-items-center rounded-full transition-colors',
                  tab.active && 'bg-surface-hover',
                )}
              >
                <tab.icon className="size-5" aria-hidden="true" />
              </span>
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
          className="gap-0 rounded-t-2xl px-2 pt-2 pb-[calc(8px+var(--pisper-safe-area-bottom))]"
        >
          <div
            className="mx-auto mb-1 h-1 w-9 rounded-full bg-[var(--stroke)]"
            aria-hidden="true"
          />
          <SheetHeader className="px-3 pt-2 pb-1">
            <SheetTitle className="text-[13px] text-muted-foreground">
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
                  className="flex min-h-12 items-center gap-3 rounded-xl border-0 bg-transparent px-3 text-left text-[15px] text-content hover:bg-surface-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-[var(--focus)] disabled:opacity-40"
                  onClick={() => {
                    setSheetGroupId(null)
                    if (!current) onSelect(item.id)
                  }}
                >
                  <item.icon className="size-5 shrink-0 text-muted-foreground" aria-hidden="true" />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  {current && <Check className="size-4 shrink-0" aria-hidden="true" />}
                </button>
              )
            })}
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}
