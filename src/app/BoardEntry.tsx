// 会话页的看板入口（主页改为会话之后取代“‹ 看板”返回键）：看板图标 + “看板”，语义是“打开看板”，
// 点它 push 到 /board，浏览器后退回到会话。桌面放在泳道条最左侧并标出“全部代理”快捷键（openBoard，⌘1；
// v2 的 showBoard 已退役，⇧⌘B 归会话树）；
// 手机放进会话顶栏左侧，写作“看板 ▲N”，带琥珀的需要你计数（规格 7.3 的徽标保留）。
// 只认点击与应用级快捷键，不监听 Esc：键盘 Esc 永远不离开会话页。
import { LaneBarsIcon } from '@/components/app/LaneBarsIcon'
import { useI18n } from '@/app/use-i18n'
import { Kbd } from '@/components/ui/kbd'
import { useFleetSummary } from '@/features/board/public'
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
  const fleet = useFleetSummary()
  const attention = phone ? fleet.attention : 0
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
