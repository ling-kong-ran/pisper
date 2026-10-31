// 派发浮层：按 ⌘N 或点“新任务”时出现，字段与看板启动台相同（项目、模型 + 思考档位、
// 审批模式、Plan 模式）。桌面是 640px 的对话框（方向 A，效果图 extra-dispatch-light.png）：
// 顶部一行“新任务 … Esc”，无框输入区，芯片工具行，底部一条细线隔开的说明；手机外壳是带拖拽条的底部表单。
// ↵ 派发后关闭浮层、留在当前页；⌘↵ 派发后打开该会话。运行流由看板 store 持有，关闭浮层不影响运行。
import { Info } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogTitle,
} from '@/components/ui/dialog'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { LaunchConsole } from './LaunchConsole'

export type DispatchOverlayProps = {
  open: boolean
  phone: boolean
  onOpenChange: (open: boolean) => void
  // Runtime 受理后回调：open 表示用户按了 ⌘↵，宿主负责打开会话；否则只需提示已派发。
  onDispatched: (sessionId: string, options: { open: boolean }) => void
  // 浮层在发送途中被关闭、随后派发失败：浮层已卸载，交给宿主用全局提示报错（草稿已放回）。
  onDetachedFailure: (message: string) => void
  onConfigureModels: () => void
}

export function DispatchOverlay({
  open,
  phone,
  onOpenChange,
  onDispatched,
  onDetachedFailure,
  onConfigureModels,
}: DispatchOverlayProps) {
  const { t } = useI18n()
  const consoleBody = (
    <LaunchConsole
      variant={phone ? 'sheet' : 'overlay'}
      autoFocus
      onDispatched={onDispatched}
      onDetachedFailure={onDetachedFailure}
      onConfigureModels={() => {
        onOpenChange(false)
        onConfigureModels()
      }}
    />
  )
  if (phone)
    return (
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          aria-describedby="board-dispatch-description"
          className="max-h-[90dvh] gap-3 overflow-y-auto rounded-t-(--r-sheet) px-4 pt-2 pb-[calc(16px+var(--pisper-safe-area-bottom))]"
        >
          <span
            aria-hidden="true"
            className="mx-auto h-1 w-9 flex-none rounded-full bg-stroke-strong"
          />
          <SheetHeader className="p-0">
            <SheetTitle>{t('board:dispatch.title')}</SheetTitle>
            <SheetDescription id="board-dispatch-description">
              {t('board:dispatch.description')}
            </SheetDescription>
          </SheetHeader>
          {consoleBody}
        </SheetContent>
      </Sheet>
    )
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        aria-describedby="board-dispatch-description"
        data-dispatch-dialog
        // 640 宽、r-lg、实心底（规格 6.4）；上缘在视口 1/7 处，不压在页面正中，长任务描述向下长。
        className="top-[max(16px,13vh)] max-w-[min(92vw,640px)] translate-y-0 gap-0 overflow-hidden rounded-[var(--r-lg)] border border-stroke bg-solid p-0"
      >
        <div className="flex items-center gap-3 px-5 pt-4 pb-2">
          <DialogTitle className="min-w-0 flex-1 text-[15px] leading-6 font-semibold">
            {t('board:dispatch.title')}
          </DialogTitle>
          {/* 关闭：只写“Esc”的轻量文字键（效果图），读屏名是“关闭”。 */}
          <DialogClose
            aria-label={t('common:ui.closeDialog')}
            className="-mr-1.5 h-7 rounded-[var(--r-sm)] px-1.5 text-xs text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-focus pointer-coarse:h-11 pointer-coarse:min-w-11"
          >
            Esc
          </DialogClose>
        </div>
        {consoleBody}
        <div className="flex min-w-0 items-center gap-2 border-t border-stroke-soft bg-surface-subtle px-5 py-2.5 text-xs text-content-muted">
          <Info className="size-3.5 flex-none" aria-hidden="true" />
          <DialogDescription
            id="board-dispatch-description"
            className="min-w-0 flex-1 text-xs text-content-muted"
          >
            {t('board:dispatch.description')}
          </DialogDescription>
          <span className="flex-none max-[700px]:hidden">{t('board:dispatch.hint')}</span>
        </div>
      </DialogContent>
    </Dialog>
  )
}
