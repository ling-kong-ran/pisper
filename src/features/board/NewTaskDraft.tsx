// 会话主页的“新任务”草稿（主页改为会话）：会话页外壳 + 空转录 + 输入框。上次停留的会话不存在、
// 已归档或已删除时由应用壳的主页显示。项目、模型、审批与运行模式沿用启动台（含登记的空项目与
// pisper-launch-defaults）；进入草稿不建任何会话，发出第一条消息时才建，宿主随即以 replace 打开它。
// 桌面：会话页头 44 + 空转录 + 768 宽的输入框；手机：56 顶栏（左侧看板入口）+ 代理条 + 停靠底部的输入框，
// 底部安全区由这里让出，软键盘弹起时沿用 MobileViewportStabilizer 随可视视口移动。
// 草稿本身就是“新任务”：挂载期间认领 ⌘N 与侧栏“新任务”的请求（见 new-task.ts），聚焦草稿而不叠派发浮层。
import { useRef, type ReactNode } from 'react'
import { useI18n } from '@/app/use-i18n'
import { SessionBubbleIcon } from '@/components/app/LaneBarsIcon'
import { LaunchConsole, type LaunchConsoleHandle } from './LaunchConsole'
import { useBoardNewTaskHandler, type BoardNewTaskSource } from './new-task'

// 草稿认领的来源：⌘N（随改绑）与标着同一个 ⌘N 的侧栏“新任务”入口，二者在草稿上做同一件事。
// 命令面板的“新会话”照旧打开派发浮层：它在面板对话框里执行，对话框关闭时把焦点还回打开前的位置，
// 在这里聚焦草稿会被立刻撤销，命令看起来就什么也没做。手机标签栏在主页隐藏，不会发来请求。
const DRAFT_CLAIMED_SOURCES: ReadonlySet<BoardNewTaskSource> = new Set(['shortcut', 'sidebar'])

export type NewTaskDraftProps = {
  phone: boolean
  // 手机顶栏左侧（应用壳的看板入口）与顶栏下方的代理条；桌面不传，入口与代理都在应用侧栏里。
  leading?: ReactNode
  strip?: ReactNode
  // 第一条消息被 Runtime 受理：新会话 id。
  onDispatched: (sessionId: string) => void
  // 发送途中草稿已卸载（例如切去了看板）时的失败：草稿已放回，由宿主用全局提示报错。
  onDetachedFailure: (message: string) => void
  onConfigureModels: () => void
}

export function NewTaskDraft({
  phone,
  leading,
  strip,
  onDispatched,
  onDetachedFailure,
  onConfigureModels,
}: NewTaskDraftProps) {
  const { t } = useI18n()
  const consoleRef = useRef<LaunchConsoleHandle>(null)
  // 与看板认领 ⌘N 聚焦启动台同一条路径：应用壳只有一个按键入口（AppShortcuts 的 primaryAction 绑定，改绑照常
  // 生效），新任务请求先问当前页有没有自己的输入框。认领时聚焦草稿并把光标放到末尾，不在新任务草稿上
  // 再叠一个新任务浮层。手机草稿不自动聚焦是为了落地时不弹软键盘；硬件键盘的 ⌘N 是明确操作，照样聚焦。
  useBoardNewTaskHandler(({ source }) => {
    if (!DRAFT_CLAIMED_SOURCES.has(source) || !consoleRef.current) return false
    consoleRef.current.focusEnd()
    return true
  })
  const intro = (
    <div className="flex min-w-0 flex-col items-start gap-2" data-new-task-intro>
      <span
        aria-hidden="true"
        className="grid size-9 place-items-center rounded-md border border-stroke bg-solid text-content-muted shadow-edge"
      >
        <SessionBubbleIcon className="size-[18px]" strokeWidth={1.75} />
      </span>
      <h2 className="text-xl font-semibold tracking-[-.01em] text-content max-[650px]:text-lg">
        {t('board:home.draftHeading')}
      </h2>
      <p className="text-sm leading-6 text-content-muted">{t('board:home.draftHint')}</p>
    </div>
  )
  const composer = (
    <LaunchConsole
      variant="composer"
      ref={consoleRef}
      touch={phone}
      compactChips={phone}
      // 手机上自动聚焦会立刻弹出软键盘，挡住草稿说明；桌面直接可以输入。
      autoFocus={!phone}
      onDispatched={(sessionId) => onDispatched(sessionId)}
      onDetachedFailure={onDetachedFailure}
      onConfigureModels={onConfigureModels}
    />
  )

  if (phone)
    return (
      <div
        className="flex min-h-0 min-w-0 flex-1 flex-col bg-panel pb-[var(--pisper-safe-area-bottom)] [[data-mobile-keyboard='open']_&]:pb-0"
        data-new-task-draft="phone"
      >
        <header
          data-session-header="draft"
          data-focus-region="header"
          className="flex h-14 min-w-0 shrink-0 items-center gap-1 bg-chrome px-1"
        >
          <div className="flex flex-none items-center">{leading}</div>
          <div className="flex min-w-0 flex-1 flex-col items-center justify-center">
            <h1 className="max-w-full truncate px-1.5 text-[15px] leading-5 font-semibold text-content">
              {t('board:home.draftTitle')}
            </h1>
            <p className="max-w-full truncate text-xs leading-5 text-content-muted">
              {t('board:home.draftSubtitle')}
            </p>
          </div>
          {/* 与左侧入口等宽的占位，让标题在顶栏里居中。 */}
          <span aria-hidden="true" className="w-11 flex-none" />
        </header>
        {strip}
        <div className="flex min-h-0 flex-1 flex-col justify-end overflow-y-auto px-4 pt-4 pb-5">
          {intro}
        </div>
        <div data-focus-region="composer" className="shrink-0 px-3 pt-1 pb-2.5">
          {composer}
        </div>
      </div>
    )

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-panel" data-new-task-draft="desktop">
      <header
        data-session-header="draft"
        data-focus-region="header"
        data-window-drag-region
        className="flex h-[52px] min-w-0 shrink-0 items-center gap-2 border-b border-stroke-soft pr-2 pl-4 [[data-custom-titlebar]_&]:pr-[144px]"
      >
        <h1 className="flex-none text-[15px] leading-6 font-semibold text-content">
          {t('board:home.draftTitle')}
        </h1>
        <span className="min-w-0 truncate text-xs text-content-muted">
          {t('board:home.draftSubtitle')}
        </span>
      </header>
      <div className="flex min-h-0 flex-1 flex-col justify-end overflow-y-auto">
        <div className="mx-auto w-[min(768px,calc(100%_-_48px))] pt-8 pb-6">{intro}</div>
      </div>
      <div
        data-focus-region="composer"
        className="mx-auto w-[min(768px,calc(100%_-_48px))] shrink-0 pt-2 pb-3"
      >
        {composer}
      </div>
    </div>
  )
}
