// 已停止挂载：主页改为会话后，会话页的看板入口换成 BoardEntry（打开看板，不再是返回）；本文件留到第 6 期删除。
// ⌘[“返回看板”动作已退役，这里不再显示快捷键，只保留按钮本身以便原样删除。
// 会话页的返回入口（第 1 期过渡）：“← 看板”。不再单独占一行——由会话顶栏放在最左侧
// （桌面在工作区 / 标题之前，手机放进顶栏左侧留出的位置），标题栏 + 会话顶栏合计 96px。
// 手机上带琥珀的需要你计数（规格 7.3）。第 3 期由泳道条取代；Esc 不用于返回，只认按钮与浏览器后退。
import { ArrowLeft, ChevronLeft } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useFleetSummary } from '@/features/board/public'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { cn } from '@/lib/utils'

type SessionReturnBarProps = {
  onBack: () => void
}

export function SessionReturnBar({ onBack }: SessionReturnBarProps) {
  const { t } = useI18n()
  const phone = usePhoneShell()
  const fleet = useFleetSummary()
  const attention = phone ? fleet.attention : 0
  const label =
    attention > 0
      ? t('navigation:shell.backToBoardAttention', { count: attention })
      : t('navigation:shell.backToBoardLabel')
  return (
    <button
      type="button"
      data-session-back
      className={cn(
        'flex flex-none items-center gap-1.5 rounded-sm text-sm font-medium text-content-secondary transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus',
        phone ? 'relative h-11 min-w-11 px-1.5 text-base' : 'h-8 px-2 pointer-coarse:h-11',
      )}
      aria-label={label}
      onClick={onBack}
    >
      {phone ? (
        <ChevronLeft className="size-5" aria-hidden="true" />
      ) : (
        <ArrowLeft className="size-4" aria-hidden="true" />
      )}
      {t('navigation:shell.backToBoard')}
      {attention > 0 && (
        <span
          aria-hidden="true"
          className="grid h-[18px] min-w-[18px] place-items-center rounded-full bg-st-attn-solid px-1 font-mono text-xs leading-none font-semibold text-on-attn tabular-nums ring-1 ring-st-attn-ring ring-inset"
        >
          {attention}
        </span>
      )}
    </button>
  )
}
