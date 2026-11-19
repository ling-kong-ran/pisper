// 桌面程序更新的状态镜像：轮询壳的下载状态，并把“检查 / 下载 / 安装 / 自动下载开关”转交给壳。
// 状态的唯一所有者是壳（shell_installer.rs），这里不自己推断下载结果。
// 只在带 shellInstaller 桥接的新壳上按需加载，不进入启动入口包。
import type {
  ShellInstallerBridge,
  ShellInstallerController,
  ShellInstallerStatus,
} from '@/types/update'
import { normalizeShellInstallerStatus, shellInstallerPollDelay } from './shell-installer-state'

type Scheduler = Pick<Window, 'setTimeout' | 'clearTimeout'>

const METHODS = ['status', 'check', 'download', 'setAutoDownload'] as const

// 旧壳没有这组桥接，或只带了一部分方法：逐个确认是函数才用。安装入口二选一：
// 现行的壳有 install（重启并更新），v0.6.40 的壳只有 open（用系统打开安装包）。
export function usableShellInstallerBridge(value: unknown): ShellInstallerBridge | null {
  if (!value || typeof value !== 'object') return null
  const bridge = value as Record<string, unknown>
  return METHODS.every((name) => typeof bridge[name] === 'function') &&
    (typeof bridge.install === 'function' || typeof bridge.open === 'function')
    ? (value as ShellInstallerBridge)
    : null
}

// 开始镜像；每拿到一份新状态就调用 publish（平台不支持时给 null）。返回停止函数，停止后不再发请求。
export function startShellInstallerPoller(
  source: unknown,
  publish: (controller: ShellInstallerController | null) => void,
  timers?: Scheduler,
) {
  const bridge = usableShellInstallerBridge(source)
  if (!bridge) return () => {}
  const scheduler = timers ?? window
  let stopped = false
  let sequence = 0
  let timer: number | null = null
  let status: ShellInstallerStatus | null = null

  const schedule = () => {
    if (timer !== null) scheduler.clearTimeout(timer)
    timer = null
    if (stopped || !status || status.state === 'unsupported') return
    // 进行中每秒刷新进度，其余时候低频刷新，好看到壳的定时任务开始的下载。
    timer = scheduler.setTimeout(() => void refresh(), shellInstallerPollDelay(status.state))
  }

  // 只采用最后一次发出的请求的结果，避免慢的旧响应覆盖新状态。
  const request = async (call: () => Promise<unknown>, rethrow = false) => {
    const current = ++sequence
    try {
      const value = await call()
      if (stopped || current !== sequence) return
      const next = normalizeShellInstallerStatus(value)
      if (next) {
        status = next
        publish(next.supported ? { status: next, installKind, ...actions } : null)
      }
      schedule()
    } catch (error) {
      // 失败时保留上一份状态，按原节奏继续刷新；用户操作的失败交给界面提示。
      if (!stopped && current === sequence) schedule()
      if (rethrow) throw error
    }
  }

  const refresh = () => request(() => bridge.status())
  const installKind = typeof bridge.install === 'function' ? 'restart' : 'installer'
  const installCall = bridge.install ?? bridge.open

  const actions = {
    check: () => request(() => bridge.check(), true),
    download: () => request(() => bridge.download(), true),
    setAutoDownload: (enabled: boolean) => request(() => bridge.setAutoDownload(enabled), true),
    // 安装前壳会再验一次签名；重启并更新成功时 Pisper 直接退出，失败才回到这里刷新状态。
    install: async () => {
      try {
        return (await installCall?.()) === true
      } finally {
        void refresh()
      }
    },
  }

  void refresh()
  return () => {
    stopped = true
    if (timer !== null) scheduler.clearTimeout(timer)
    timer = null
  }
}
