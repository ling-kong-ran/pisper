// 手机外壳顶栏左侧的 ☰（规格 v2 第 7.1 节）：点开抽屉；需要你 >0 时旁边常驻琥珀“● N”，
// 离开全部代理也不会错过等你的事项。计数读侧栏同一份分组模型（看板 store），不新开轮询。
// 整块是一个 44px 高的按钮（☰ 与计数一起），读屏读出“打开导航，3 个需要你”。
// 本模块读看板数据层，只随会话路由或手机顶栏懒加载，不进入口包。
import { Menu } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { useSidebarModel } from '@/features/board/public'
import { cn } from '@/lib/utils'
import { useSidebarStore } from '@/stores/sidebar-store'

export function PhoneMenuButton({ className }: { className?: string }) {
  const { t } = useI18n()
  const { model } = useSidebarModel()
  const open = useSidebarStore((state) => state.drawerOpen)
  const setOpen = useSidebarStore((state) => state.setDrawerOpen)
  const attention = model.counts.attention
  return (
    <button
      type="button"
      data-phone-menu
      aria-haspopup="dialog"
      aria-expanded={open}
      aria-label={
        attention > 0
          ? t('navigation:phone.menuAttention', { count: attention })
          : t('navigation:phone.menu')
      }
      className={cn(
        'flex h-11 min-w-11 flex-none items-center gap-1.5 rounded-[var(--r-md)] px-2.5 text-content transition-colors duration-(--d-fast) focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus active:bg-surface-hover',
        className,
      )}
      onClick={() => setOpen(true)}
    >
      <Menu className="size-5" strokeWidth={1.75} aria-hidden="true" />
      {attention > 0 && (
        <span
          aria-hidden="true"
          data-phone-attention={attention}
          className="flex h-5 items-center gap-1 rounded-full bg-warning-soft px-1.5 text-xs leading-none font-semibold text-warning-strong tabular-nums ring-1 ring-st-attn-border ring-inset"
        >
          <span className="size-1.5 rounded-full bg-st-attn-fill" />
          {attention > 99 ? '99+' : attention}
        </span>
      )}
    </button>
  )
}
