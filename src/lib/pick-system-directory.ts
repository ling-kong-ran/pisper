// 选择工作目录一律交给系统文件管理器：桌面壳走 Tauri 桥接，同机浏览器由本机 Runtime 弹出原生对话框。
// 只有系统对话框确实无法出现（配对设备、无图形会话的主机、手机 Runtime）时才返回 unavailable，
// 由调用方退回手动填写 Runtime 路径。
import { apiJson } from '@/lib/api'

export type SystemDirectoryPick =
  { status: 'picked'; path: string } | { status: 'cancelled' } | { status: 'unavailable' }

type RuntimePickResponse = { status?: string; path?: string }

/** Open the operating system folder picker and return an absolute Runtime path. */
export async function pickSystemDirectory(
  initialDirectory = '',
  title = '',
): Promise<SystemDirectoryPick> {
  const pickDirectory = window.pisperDesktop?.pickDirectory
  if (pickDirectory) {
    const path = await pickDirectory(initialDirectory || undefined)
    return path ? { status: 'picked', path } : { status: 'cancelled' }
  }
  let result: RuntimePickResponse
  try {
    result = await apiJson<RuntimePickResponse>('/api/local/directory-picker', {
      method: 'POST',
      body: JSON.stringify({ initialDirectory, title }),
    })
  } catch {
    // 旧版或经代理访问的 Runtime 没有这个接口（404 等），同样退回手动填写，而不是报错中断新建项目。
    return { status: 'unavailable' }
  }
  if (result.status === 'picked' && result.path) return { status: 'picked', path: result.path }
  // busy 表示系统对话框已经打开，本次点击不再叠加窗口。
  if (result.status === 'cancelled' || result.status === 'busy') return { status: 'cancelled' }
  return { status: 'unavailable' }
}
