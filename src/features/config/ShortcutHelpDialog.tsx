// @public ⌘/ 快捷键速查：按“全局 / 看板 / 会话”分组列出当前绑定（读 shortcut-store，改绑后立即反映）。
// 由应用壳按需加载，打开即挂载、关闭即卸载；Esc 或关闭键收起后焦点回到打开前的位置。
import { useRef, useState } from 'react'
import { Settings } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { useShortcutStore } from '@/stores/shortcut-store'
import {
  SHORTCUT_GROUPS,
  shortcutEntryKey,
  shortcutEntryKeys,
  useShortcutCatalogText,
} from './shortcut-catalog'
import { ShortcutKeys } from './ShortcutKeys'

export function ShortcutHelpDialog({
  open,
  onOpenChange,
  onCustomize,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  // 跳到“设置 → 快捷键”；不传时不显示该按钮。
  onCustomize?: () => void
}) {
  const { t } = useI18n()
  const text = useShortcutCatalogText()
  const bindings = useShortcutStore((state) => state.bindings)
  const boardBindings = useShortcutStore((state) => state.boardBindings)
  // 速查由快捷键打开、没有触发按钮：Radix 默认把焦点还给（不存在的）触发器，会落到 body。
  // 挂载时记下当时的焦点（通常是输入框或看板卡片），关闭时还回去。
  const [returnTo] = useState(() =>
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  )
  // 打开时把焦点放在可滚动的列表上：方向键 / 翻页键直接滚动，不会先落到“在设置中改绑”上被回车误触。
  const listRef = useRef<HTMLDivElement>(null)
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        size="wide"
        scrollMode="body"
        className="max-w-[min(92vw,760px)]"
        data-shortcut-help
        onOpenAutoFocus={(event) => {
          if (!listRef.current) return
          event.preventDefault()
          listRef.current.focus({ preventScroll: true })
        }}
        onCloseAutoFocus={(event) => {
          if (!returnTo?.isConnected) return
          event.preventDefault()
          returnTo.focus({ preventScroll: true })
        }}
      >
        <DialogHeader className="pr-8">
          <DialogTitle>{t('board:shortcuts.shortcutHelp')}</DialogTitle>
          <DialogDescription>{t('board:shortcuts.helpDescription')}</DialogDescription>
        </DialogHeader>
        <DialogBody
          ref={listRef}
          tabIndex={0}
          role="region"
          aria-label={t('board:shortcuts.shortcutHelp')}
          // 键盘打开时焦点落在列表上：描边内收并留出上下边距，不压到分组标题。
          className="-mx-4 px-4 py-2 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus"
        >
          <div className="grid gap-x-8 gap-y-5 min-[720px]:grid-cols-2">
            {SHORTCUT_GROUPS.map((group) => (
              <section
                key={group.id}
                aria-labelledby={`shortcut-help-${group.id}`}
                data-shortcut-help-group={group.id}
                // 看板分组最长，单独占一列；全局与会话在另一列上下排。
                className={group.id === 'board' ? 'min-[720px]:row-span-2' : undefined}
              >
                <h3
                  id={`shortcut-help-${group.id}`}
                  className="text-[13px] leading-5 font-semibold text-content"
                >
                  {text.groups[group.id].title}
                </h3>
                <p className="mb-1 text-xs text-content-muted">{text.groups[group.id].hint}</p>
                <dl className="divide-y divide-stroke-soft">
                  {group.entries.map((entry) => (
                    <div
                      key={shortcutEntryKey(entry)}
                      className="flex min-h-7 min-w-0 items-center justify-between gap-3 py-0.5"
                      data-shortcut-help-entry={shortcutEntryKey(entry)}
                    >
                      <dt className="min-w-0 text-[13px] text-content">{text.label(entry)}</dt>
                      <dd className="flex-none">
                        <ShortcutKeys
                          keys={shortcutEntryKeys(entry, bindings, boardBindings)}
                          className="justify-end"
                        />
                      </dd>
                    </div>
                  ))}
                </dl>
              </section>
            ))}
          </div>
        </DialogBody>
        {onCustomize && (
          <DialogFooter>
            <Button
              variant="outline"
              size="dialogAction"
              onClick={() => {
                onOpenChange(false)
                onCustomize()
              }}
            >
              <Settings aria-hidden="true" />
              {t('board:shortcuts.helpCustomize')}
            </Button>
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  )
}
