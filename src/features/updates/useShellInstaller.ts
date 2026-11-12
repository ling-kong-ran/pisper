// 桌面壳安装包控制器：新壳带 shellInstaller 桥接时按需加载状态镜像（shell-installer-poller.ts），
// 浏览器、手机和旧壳上什么都不加载，始终为 null。入口包只留这一小段。
import { useEffect, useState } from 'react'
import type { ShellInstallerController } from '@/types/update'

export function useShellInstaller(source: unknown): ShellInstallerController | null {
  const [controller, setController] = useState<ShellInstallerController | null>(null)
  useEffect(() => {
    if (!source || typeof source !== 'object') return
    let active = true
    let stop = () => {}
    void import('./shell-installer-poller')
      .then(({ startShellInstallerPoller }) => {
        if (active) stop = startShellInstallerPoller(source, setController)
      })
      .catch(() => {})
    return () => {
      active = false
      stop()
      setController(null)
    }
  }, [source])
  return controller
}
