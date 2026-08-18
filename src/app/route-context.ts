// 路由 Outlet 上下文类型：App 壳通过 useOutletContext 向所有页面注入
// 导航、通知、对话框、更新控制器等公共能力，页面只声明自己需要的接口。
import type { ToastTone } from '@/components/ui/toast'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import type { ChatAttachment, PendingAsset } from '@/types/chat'
import type { NotificationSettingsData } from '@/types/notifications'
import type { AppUpdateController } from '@/types/update'
import type { WorkflowActions } from '@/types/workflow'

export type Notify = (message: string, tone?: ToastTone) => void

export type AppRouteContext = {
  query: string
  activeSessionId: string
  navigate: (page: string, options?: { replace?: boolean }) => void
  notify: Notify
  browserNotify: (event: string, data: unknown, options?: { force?: boolean }) => void
  registerPrimaryAction: (action: () => void) => () => void
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
  setPluginStats: (stats: { enabled: number; total: number } | null) => void
  registerWorkflowActions: (actions: WorkflowActions) => () => void
}
