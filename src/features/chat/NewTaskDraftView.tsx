// 新任务草稿在会话页里的两块专属画面：页头与转录区的问候语。桌面页头写“新任务 · 发出第一条消息时才会创建会话”；
// 手机照参考稿只写“新任务”，下面一行是项目选择（和会话页顶栏写项目名的位置一致）。
// 其余（输入框、模型与审批芯片、斜杠命令）就是会话页本身，见 FocusSession 的 draft。
import { useEffect, useState, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { BrandLogo } from '@/components/BrandLogo'

// 问候语按本地时间分段，跨过分段边界时自己换掉（长时间停在草稿上也不会一直说“早上好”）。
function greetingPeriod(date: Date) {
  const hour = date.getHours()
  if (hour >= 5 && hour < 11) return 'morning'
  if (hour >= 11 && hour < 13) return 'noon'
  if (hour >= 13 && hour < 18) return 'afternoon'
  if (hour >= 18 && hour < 23) return 'evening'
  return 'lateNight'
}

function msUntilNextHour(date: Date) {
  const next = new Date(date)
  next.setHours(date.getHours() + 1, 0, 0, 0)
  return Math.max(1_000, next.getTime() - date.getTime())
}

export function NewTaskGreeting() {
  const { t } = useI18n()
  const [now, setNow] = useState(() => new Date())
  useEffect(() => {
    const timer = window.setTimeout(() => setNow(new Date()), msUntilNextHour(now))
    return () => window.clearTimeout(timer)
  }, [now])
  const period = greetingPeriod(now)
  const greeting =
    period === 'morning'
      ? t('chat:newTask.greetingMorning')
      : period === 'noon'
        ? t('chat:newTask.greetingNoon')
        : period === 'afternoon'
          ? t('chat:newTask.greetingAfternoon')
          : period === 'evening'
            ? t('chat:newTask.greetingEvening')
            : t('chat:newTask.greetingLateNight')
  return (
    // 手机：问候语停在输入框上方区域的正中，不再贴在左下角；那段说明只在桌面出现（手机上太长）。
    <div
      className="grid min-h-full content-center justify-items-center gap-2 text-center max-[650px]:gap-3"
      data-new-task-greeting
    >
      <BrandLogo size={32} />
      {/* 会话页的转录区把 h2 统一压成正文字号（[&_h2]:text-base）：问候语用 ! 保住自己的字号。 */}
      <h2 className="text-2xl! font-semibold tracking-[-.01em] text-balance text-content max-[650px]:text-xl!">
        {greeting}
      </h2>
      <p className="max-w-[46ch] text-sm leading-6 text-content-muted max-[650px]:hidden">
        {t('board:home.draftHint')}
      </p>
    </div>
  )
}

export function NewTaskDraftHeader({
  phone,
  leading,
  project,
}: {
  phone: boolean
  leading?: ReactNode
  // 手机顶栏标题下的项目选择（由看板提供）。
  project?: ReactNode
}) {
  const { t } = useI18n()
  if (phone)
    return (
      <header
        data-session-header="draft"
        data-focus-region="header"
        className="grid h-14 min-w-0 shrink-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] items-center bg-panel px-3"
      >
        <div className="flex min-w-0 items-center">{leading}</div>
        <div className="flex max-w-[min(58vw,260px)] min-w-0 flex-col items-center justify-center">
          <h1 className="max-w-full truncate px-1.5 text-base leading-5 font-semibold text-content">
            {t('board:home.draftTitle')}
          </h1>
          {project}
        </div>
        {/* 三栏网格让标题在顶栏里居中；右栏留空（这里本身就是新任务）。 */}
        <span aria-hidden="true" />
      </header>
    )
  return (
    <header
      data-session-header="draft"
      data-focus-region="header"
      data-window-drag-region
      className="flex h-[52px] min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft pr-2 pl-4 [[data-custom-titlebar]_&]:pr-[144px]"
    >
      {leading}
      <h1 className="flex-none text-[15px] leading-6 font-semibold text-content">
        {t('board:home.draftTitle')}
      </h1>
      <span className="min-w-0 truncate text-xs text-content-muted">
        {t('board:home.draftSubtitle')}
      </span>
    </header>
  )
}
