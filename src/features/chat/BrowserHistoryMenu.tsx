// 右侧浏览器的“历史记录”：地址栏旁的图标键打开可搜索列表（Popover + Command），
// 按今天 / 昨天 / 7 天内 / 更早分组，选中后在当前页签打开；底部可清除（先确认）。
// 弹层和确认框盖住原生 WebView 时由 useNativeBrowser 的遮挡检测把网页藏起来。
import { useRef, useState } from 'react'
import { History, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState } from '@/components/app/PageState'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import {
  browserHistoryHost,
  groupBrowserHistory,
  type BrowserHistoryGroup,
} from './browser-history'
import { useBrowserHistoryStore } from './browser-history-store'

export function BrowserHistoryMenu({ onOpen }: { onOpen: (url: string) => void }) {
  const { t } = useI18n()
  const entries = useBrowserHistoryStore((state) => state.entries)
  const [open, setOpen] = useState(false)
  const [confirming, setConfirming] = useState(false)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const label = t('common:webPreview.history')
  const headings: Record<BrowserHistoryGroup, string> = {
    today: t('common:webPreview.historyToday'),
    yesterday: t('common:webPreview.historyYesterday'),
    week: t('common:webPreview.historyWeek'),
    earlier: t('common:webPreview.historyEarlier'),
  }
  // 只在打开时分组：关着的时候历史变化不必重算。
  const groups = open ? groupBrowserHistory(entries, Date.now()) : []

  return (
    <>
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            ref={triggerRef}
            type="button"
            variant="ghost"
            size="icon"
            aria-label={label}
            title={label}
          >
            <History size={14} />
          </Button>
        </PopoverTrigger>
        <PopoverContent align="end" className="w-80 gap-0 p-0">
          {entries.length ? (
            <Command className="bg-transparent">
              <CommandInput placeholder={t('common:webPreview.historySearch')} />
              <CommandList className="max-h-80">
                <CommandEmpty className="py-0">
                  <InlineEmptyState compact title={t('common:webPreview.historyNoMatch')} />
                </CommandEmpty>
                {groups.map((group) => (
                  <CommandGroup key={group.key} heading={headings[group.key]}>
                    {group.entries.map((entry) => {
                      const host = browserHistoryHost(entry.url)
                      return (
                        <CommandItem
                          key={entry.url}
                          value={entry.url}
                          keywords={entry.title ? [entry.title] : undefined}
                          title={entry.url}
                          onSelect={() => {
                            setOpen(false)
                            onOpen(entry.url)
                          }}
                        >
                          <span className="flex min-w-0 flex-1 flex-col">
                            <span className="truncate">{entry.title || host}</span>
                            <span className="truncate text-xs text-content-muted">{host}</span>
                          </span>
                        </CommandItem>
                      )
                    })}
                  </CommandGroup>
                ))}
              </CommandList>
              <div className="border-t border-stroke-soft p-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="w-full justify-start text-danger"
                  onClick={() => {
                    setOpen(false)
                    setConfirming(true)
                  }}
                >
                  <Trash2 size={14} />
                  {t('common:webPreview.clearHistory')}
                </Button>
              </div>
            </Command>
          ) : (
            <InlineEmptyState compact icon={History} title={t('common:webPreview.historyEmpty')} />
          )}
        </PopoverContent>
      </Popover>
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent
          onCloseAutoFocus={(event) => {
            // 确认框是从弹层里打开的，没有自己的触发键：关上后焦点回到历史记录键。
            event.preventDefault()
            triggerRef.current?.focus()
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>{t('common:webPreview.clearHistoryTitle')}</AlertDialogTitle>
            <AlertDialogDescription>
              {t('common:webPreview.clearHistoryDescription')}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t('common:ui.cancel')}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => useBrowserHistoryStore.getState().clear()}
            >
              {t('common:webPreview.clearHistoryConfirm')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
