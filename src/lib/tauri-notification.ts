// Tauri 本地通知通道：经 core.invoke 直调 notification 插件命令
// （与 connect 页调扫码插件同一模式）。与浏览器 Notification/SW 通道并列，
// 仅在 Tauri 壳内可用；桌面壳有 pisperDesktop 桥接，优先级更高。

function tauriInvoke() {
  return window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
}

export function tauriNotificationAvailable(): boolean {
  return typeof window !== 'undefined' && Boolean(tauriInvoke())
}

function invoke<T>(command: string, args?: unknown): Promise<T> {
  const call = tauriInvoke()
  if (!call) return Promise.reject(new Error('Tauri IPC is unavailable.'))
  return call<T>(command, args)
}

// 插件返回 boolean | null：true=已授权，false=被拒绝，null=未询问过。
export async function tauriNotificationIsGranted(): Promise<boolean | null> {
  return invoke<boolean | null>('plugin:notification|is_permission_granted')
}

// 申请通知权限：返回 granted/denied/prompt/prompt-with-rationale。
export async function tauriNotificationRequestPermission(): Promise<string> {
  return invoke<string>('plugin:notification|request_permission')
}

export async function tauriNotificationNotify(title: string, body: string): Promise<void> {
  return invoke('plugin:notification|notify', { options: { title, body } })
}
