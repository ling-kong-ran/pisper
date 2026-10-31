// 路由 Outlet 上下文类型：App 壳通过 useOutletContext 向所有页面注入
// 导航、通知、对话框、更新控制器等公共能力，页面只声明自己需要的接口。
import type { ToastTone } from '@/components/ui/toast'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { ChatAttachment, PendingAsset } from '@/types/chat'
import type { NotificationSettingsData } from '@/types/notifications'
import type { AppUpdateController } from '@/types/update'
import type { WorkflowActions } from '@/types/workflow'

// 提示可以附带一个动作按钮（例如派发失败后的“重新打开”），点按后提示随之关闭。
export type NotifyAction = { label: string; onAction: () => void }
export type Notify = (
  message: string,
  tone?: ToastTone,
  options?: { action?: NotifyAction },
) => void

// 本地路径 reveal 结果通知事件：Markdown 链接组件只负责发射，
// 应用壳（App.tsx）统一监听并走全站 Toast，避免在 Markdown 静态闭包里
// 再引入一份 radix Toast 图（包体积预算）。
export const LOCAL_REVEAL_NOTICE_EVENT = 'pisper:local-reveal-notice'

export type AppRouteContext = {
  activeSessionId: string
  navigate: (page: string, options?: { replace?: boolean }) => void
  notify: Notify
  browserNotify: (event: string, data: unknown, options?: { force?: boolean }) => void
  registerPrimaryAction: (action: () => void) => () => void
  // 触发页面注册的主操作：设置 › 模型的“快速设置”按钮在页面自己的工具行里（A6 第 4 批，共享页头退役），
  // 注册表仍由应用壳持有——新手引导“去设置”要在模型页挂载前把快速设置排进队列。
  invokePrimaryAction: () => void
  pendingAsset: PendingAsset | null
  onAssetConsumed: () => void
  onUseAsset: (asset: ChatAttachment) => void
  requestText: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
  openNotificationSettings: () => void
  configSection: string
  setConfigSection: (section: string) => void
  setNotificationSettings: (settings: NotificationSettingsData) => void
  appUpdate: AppUpdateController
  registerWorkflowActions: (actions: WorkflowActions) => () => void
  // 新任务：看板桌面版由启动台认领并聚焦，其余情况（含手机看板）由应用壳打开派发浮层。
  startNewTask: () => void
  // 当前页属于“功能”时提供：功能页模板在标题上方渲染“‹ 功能”（规格 2.4），点它回到功能索引。
  onBackToFeatures?: () => void
}
