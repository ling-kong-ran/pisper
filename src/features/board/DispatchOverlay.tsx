// 派发浮层：在非看板页按 ⌘N 或点“新任务”时出现，字段与看板启动台相同（项目、模型 + 思考档位、
// 审批模式、Plan 模式）。桌面是 640px 居中浮层，手机外壳是带拖拽条的底部表单。
// ↵ 派发后关闭浮层、留在当前页；⌘↵ 派发后打开该会话。运行流由看板 store 持有，关闭浮层不影响运行。
import { useI18n } from '@/app/use-i18n'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
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
        aria-describedby="board-dispatch-description"
        className="max-w-[min(92vw,640px)]"
      >
        <DialogHeader>
          <DialogTitle>{t('board:dispatch.title')}</DialogTitle>
          <DialogDescription id="board-dispatch-description">
            {t('board:dispatch.description')}
          </DialogDescription>
        </DialogHeader>
        {consoleBody}
        <p className="text-xs text-content-muted">{t('board:dispatch.hint')}</p>
      </DialogContent>
    </Dialog>
  )
}
