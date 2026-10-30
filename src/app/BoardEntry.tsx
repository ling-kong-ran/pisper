// 手机会话页顶栏左侧的看板入口：看板图标 + “看板 N”，带琥珀的需要你计数（规格 7.3 的徽标保留），
// 点它 push 到 /board，浏览器后退回到会话。桌面的泳道条与这个入口已退役（入口在应用侧栏里）；
// 手机外壳在 A4 改为顶栏 ☰ + 抽屉时整个删除。计数读侧栏同一份分组模型（看板 store）。
// 只认点击与应用级快捷键，不监听 Esc：键盘 Esc 永远不离开会话页。
import { LaneBarsIcon } from '@/components/app/LaneBarsIcon'
import { useI18n } from '@/app/use-i18n'
import { Kbd } from '@/components/ui/kbd'
import { useSidebarModel } from '@/features/board/public'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { ariaKeyShortcuts } from '@/lib/shortcut-aria'
import { formatShortcut } from '@/lib/shortcuts'
import { TOUCH_HIT_AREA } from '@/lib/touch-target'
import { cn } from '@/lib/utils'
import { useShortcutStore } from '@/stores/shortcut-store'

type BoardEntryProps = {
  onOpenBoard: () => void
}

export function BoardEntry({ onOpenBoard }: BoardEntryProps) {
  const { t } = useI18n()
  const phone = usePhoneShell()
  const binding = useShortcutStore((state) => state.bindings.openBoard)
  const shortcut = formatShortcut(binding)
  const { model } = useSidebarModel()
  const attention = phone ? model.counts.attention : 0
  const label =
    attention > 0
      ? t('board:home.entryAttention', { count: attention })
      : shortcut && !phone
        ? t('board:home.entryShortcut', { shortcut })
        : t('board:home.entryLabel')
  return (
    <button
      type="button"
      data-board-entry
      title={phone ? undefined : label}
      className={cn(
        'flex flex-none items-center gap-1.5 rounded-sm text-sm font-medium text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
        // 泳道条把它压成 28px（固定条高）：触屏上可见尺寸不变，命中区用伪元素补到 44px。
        phone
          ? 'relative h-11 min-w-11 px-2 text-base'
          : cn('h-8 px-2 pointer-coarse:h-11', TOUCH_HIT_AREA),
      )}
      aria-label={label}
      aria-keyshortcuts={phone ? undefined : ariaKeyShortcuts(binding)}
      onClick={onOpenBoard}
    >
      <LaneBarsIcon className={phone ? 'size-5' : 'size-4'} strokeWidth={1.75} aria-hidden="true" />
      {t('board:home.entry')}
      {attention > 0 && (
        <span
          aria-hidden="true"
          className="grid h-[18px] min-w-[18px] place-items-center rounded-full bg-st-attn-solid px-1 font-mono text-xs leading-none font-semibold text-on-attn tabular-nums ring-1 ring-st-attn-ring ring-inset"
        >
          {attention > 99 ? '99+' : attention}
        </span>
      )}
      {shortcut && !phone && (
        <Kbd aria-hidden="true" className="max-[650px]:hidden pointer-coarse:hidden">
          {shortcut}
        </Kbd>
      )}
    </button>
  )
}
