// 应用更新控制器 hook：向壳层提供检查/下载/安装更新与
// 状态订阅的完整能力（AppUpdateController）。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { AppUpdateController, AppUpdateInfo, UpdateStatus } from '@/types/update'
import { scheduleAppUpdateChecks, shouldAutomaticallyCheckForUpdates } from './auto-update'
import { shellUpdateStatus } from './shell-installer-state'
import { checkWebUpdates, RELEASES_URL } from './update-client'
import { useShellInstaller } from './useShellInstaller'

const BUILD_VERSION = import.meta.env.VITE_APP_VERSION || '0.0.0'

const WEB_INFO: AppUpdateInfo = Object.freeze({
  desktop: false,
  packaged: false,
  version: BUILD_VERSION,
  platform: 'browser',
  arch: '',
  releasesUrl: RELEASES_URL,
})

// 应用更新控制器 hook。桌面端只整体更新桌面程序：状态、下载与安装都由壳负责（shell_installer.rs），
// 这里只做镜像；手机走 App 商店式的更新页，浏览器比对源码分支。
export function useAppUpdate(): AppUpdateController {
  const bridge = window.pisperDesktop
  const mobileInvoke = bridge
    ? undefined
    : (window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke)
  const [info, setInfo] = useState(WEB_INFO)
  const [status, setStatus] = useState<UpdateStatus>({ state: 'idle', checkedAt: null })
  const statusRef = useRef(status)
  const checkInFlightRef = useRef<Promise<UpdateStatus> | null>(null)
  const mobileModeRef = useRef(false)
  // 壳在后台检查、下载并验签更新包；v0.6.39 及更早的壳没有这组桥接，不能自更新。
  const shell = useShellInstaller(bridge?.shellInstaller)
  const shellInstaller = shell.controller
  const shellStatus = useMemo<UpdateStatus | null>(() => {
    if (!bridge) return null
    if (shellInstaller) return shellUpdateStatus(shellInstaller.status, shellInstaller.installKind)
    // 旧壳，或这个平台没有更新包（例如 Intel Mac）：只能到发布页下载安装。
    if (!bridge.shellInstaller || shell.settled) {
      return { state: 'unsupported', releaseUrl: RELEASES_URL }
    }
    return { state: 'idle' }
  }, [bridge, shell.settled, shellInstaller])

  // check 要保持稳定（手机的定时检查靠它），壳的最新状态从这里读。
  const shellRef = useRef({ controller: shellInstaller, status: shellStatus })
  useEffect(() => {
    shellRef.current = { controller: shellInstaller, status: shellStatus }
  }, [shellInstaller, shellStatus])

  const applyStatus = useCallback((value: UpdateStatus) => {
    statusRef.current = value
    setStatus(value)
  }, [])

  const check = useCallback(
    ({ refresh = true }: { refresh?: boolean } = {}) => {
      // 桌面端：壳自己定时检查；用户点“检查更新”时让壳立刻查一次，进度由状态镜像轮询。
      if (bridge) {
        const { controller } = shellRef.current
        const current = () => shellRef.current.status ?? statusRef.current
        if (!refresh || !controller) return Promise.resolve(current())
        return controller
          .check()
          .catch(() => {})
          .then(current)
      }
      if (checkInFlightRef.current) return checkInFlightRef.current

      applyStatus({ ...statusRef.current, state: 'checking', message: '' })
      const pending = (async (): Promise<UpdateStatus> => {
        try {
          if (mobileModeRef.current && mobileInvoke) {
            const next = await mobileInvoke<UpdateStatus>('mobile_check_app_update', { refresh })
            applyStatus(next)
            return next
          }
          const next = await checkWebUpdates({ refresh })
          applyStatus(next)
          return next
        } catch (error) {
          const failed = {
            state: 'error',
            message: error instanceof Error ? error.message : String(error),
            checkedAt: new Date().toISOString(),
          }
          applyStatus(failed)
          return failed
        }
      })()
      checkInFlightRef.current = pending
      void pending.finally(() => {
        if (checkInFlightRef.current === pending) checkInFlightRef.current = null
      })
      return pending
    },
    [applyStatus, bridge, mobileInvoke],
  )

  useEffect(() => {
    let active = true
    let stopAutomaticChecks = () => {}
    let stopVisibilityChecks = () => {}

    if (bridge) {
      bridge
        .getAppInfo()
        .then((value) => {
          if (active) setInfo(value)
        })
        .catch(() => {})
      return () => {
        active = false
      }
    }

    const checkWebSource = () => {
      if (!active) return
      void check({ refresh: false })
    }
    if (!mobileInvoke) {
      checkWebSource()
      return () => {
        active = false
      }
    }

    void mobileInvoke<AppUpdateInfo>('mobile_app_info')
      .then((value) => {
        if (!active) return
        if (!value.mobile) {
          checkWebSource()
          return
        }
        mobileModeRef.current = true
        setInfo(value)
        if (value.update) applyStatus(value.update)
        void check({ refresh: false })
        stopAutomaticChecks = scheduleAppUpdateChecks(() => {
          if (!shouldAutomaticallyCheckForUpdates(statusRef.current.state)) return
          return check({ refresh: false })
        })

        const checkWhenVisible = () => {
          if (document.visibilityState !== 'visible') return
          if (!shouldAutomaticallyCheckForUpdates(statusRef.current.state)) return
          void check({ refresh: false })
        }
        document.addEventListener('visibilitychange', checkWhenVisible)
        stopVisibilityChecks = () =>
          document.removeEventListener('visibilitychange', checkWhenVisible)
      })
      .catch(checkWebSource)
    return () => {
      active = false
      stopAutomaticChecks()
      stopVisibilityChecks()
    }
  }, [applyStatus, bridge, check, mobileInvoke])

  const openReleases = useCallback(async () => {
    if (bridge) return bridge.openReleases()
    const url = status.releaseUrl || info.releasesUrl || RELEASES_URL
    if (mobileModeRef.current && mobileInvoke) {
      return mobileInvoke<boolean>('mobile_open_app_update', { url })
    }
    window.open(url, '_blank', 'noopener,noreferrer')
    return true
  }, [bridge, info.releasesUrl, mobileInvoke, status.releaseUrl])

  const openUpdateLog = useCallback(() => bridge?.openUpdateLog?.(), [bridge])

  const download = useCallback(async () => {
    if (bridge) {
      if (shellInstaller?.status.canDownload) return shellInstaller.download()
      return openReleases()
    }
    if (mobileModeRef.current && mobileInvoke && status.downloadUrl) {
      return mobileInvoke<boolean>('mobile_open_app_update', { url: status.downloadUrl })
    }
    return openReleases()
  }, [bridge, mobileInvoke, openReleases, shellInstaller, status.downloadUrl])

  // 桌面端：重启并更新（v0.6.40 的壳是打开安装包）。其他平台去下载页。
  const install = useCallback(async () => {
    if (bridge && shellInstaller) return shellInstaller.install()
    return download()
  }, [bridge, download, shellInstaller])

  return {
    info,
    status: shellStatus ?? status,
    check,
    download,
    install,
    openReleases,
    openUpdateLog,
    shellInstaller,
  }
}
