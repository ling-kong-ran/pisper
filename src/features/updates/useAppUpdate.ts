// 应用更新控制器 hook：向壳层提供检查/下载/安装更新与
// 状态订阅的完整能力（AppUpdateController）。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type {
  AppUpdateController,
  AppUpdateInfo,
  ComponentUpdateStatus,
  UpdateStatus,
} from '@/types/update'
import { scheduleAppUpdateChecks, shouldAutomaticallyCheckForUpdates } from './auto-update'
import {
  componentUpdateStatus as componentStatus,
  currentDesktopVersion,
} from './component-update-state'
import { checkWebUpdates, RELEASES_URL } from './update-client'

const BUILD_VERSION = import.meta.env.VITE_APP_VERSION || '0.0.0'

const WEB_INFO: AppUpdateInfo = Object.freeze({
  desktop: false,
  packaged: false,
  version: BUILD_VERSION,
  platform: 'browser',
  arch: '',
  releasesUrl: RELEASES_URL,
})

// 应用更新控制器 hook：检查/下载/安装桌面与组件更新，维护状态并
// 订阅桌面桥接推送，向壳层提供 AppUpdateController 能力。
export function useAppUpdate(): AppUpdateController {
  const bridge = window.pisperDesktop
  const mobileInvoke = bridge
    ? undefined
    : (window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke)
  const [info, setInfo] = useState(WEB_INFO)
  const [status, setStatus] = useState<UpdateStatus>({ state: 'idle', checkedAt: null })
  const [components, setComponents] = useState<ComponentUpdateStatus[]>([])
  const statusRef = useRef(status)
  const checkInFlightRef = useRef<Promise<UpdateStatus> | null>(null)
  const legacyShellUpdateRef = useRef(false)
  const mobileModeRef = useRef(false)

  const applyStatus = useCallback((value: UpdateStatus) => {
    statusRef.current = value
    setStatus(value)
  }, [])

  const check = useCallback(
    ({ refresh = true }: { refresh?: boolean } = {}) => {
      if (checkInFlightRef.current) return checkInFlightRef.current

      applyStatus({ ...statusRef.current, state: 'checking', message: '' })
      const pending = (async (): Promise<UpdateStatus> => {
        try {
          legacyShellUpdateRef.current = false
          if (bridge?.checkForUpdates) {
            try {
              const shellUpdate = await bridge.checkForUpdates()
              if (shellUpdate.state === 'available' && shellUpdate.canDownload) {
                legacyShellUpdateRef.current = true
                applyStatus(shellUpdate)
                return shellUpdate
              }
            } catch {
              // Old Shell update checks must not block signed component updates.
            }
          }
          if (bridge?.checkComponentUpdates) {
            const items = await bridge.checkComponentUpdates()
            setComponents(items)
            const next = componentStatus(items)
            applyStatus(next)
            if (
              !items.some((item) => item.state === 'error' || item.state === 'available') &&
              items.some((item) => item.restartRequired)
            ) {
              await bridge.restartForComponentUpdate?.()
            }
            return next
          }
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

    const scheduleChecks = () => {
      stopAutomaticChecks = scheduleAppUpdateChecks(() => {
        if (!shouldAutomaticallyCheckForUpdates(statusRef.current.state)) return
        return check({ refresh: false })
      })
    }

    if (!bridge) {
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
          setComponents([])
          if (value.update) applyStatus(value.update)
          void check({ refresh: false })
          scheduleChecks()

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
    }

    bridge
      .getAppInfo()
      .then((value) => {
        if (!active) return
        setInfo({ ...value, hostVersion: value.version })
        void bridge
          .componentUpdateStatus?.()
          .then((items) => {
            if (!active) return
            setComponents(items)
            applyStatus(componentStatus(items))
          })
          .catch(() => {})
        if (value.packaged) scheduleChecks()
      })
      .catch(() => {})
    return () => {
      active = false
      stopAutomaticChecks()
    }
  }, [applyStatus, bridge, check, mobileInvoke])

  const installComponents = useCallback(async () => {
    if (legacyShellUpdateRef.current && bridge?.downloadUpdate) {
      applyStatus({
        ...statusRef.current,
        state: 'downloading',
        message: '',
        transferred: 0,
        percent: 0,
      })
      const stopStatusUpdates = bridge.onUpdateStatus?.(applyStatus)
      try {
        const downloaded = await bridge.downloadUpdate()
        applyStatus(downloaded)
        if (downloaded.state === 'downloaded' && downloaded.canInstall) {
          await bridge.installUpdate?.()
        }
      } finally {
        stopStatusUpdates?.()
      }
      return []
    }
    if (!bridge?.installComponentUpdates) return []
    applyStatus({
      ...statusRef.current,
      state: 'downloading',
      message: '',
      transferred: 0,
      percent: 0,
    })
    let pollInFlight = false
    let installFinished = false
    const refreshProgress = async () => {
      if (installFinished || pollInFlight || !bridge.componentUpdateStatus) return
      pollInFlight = true
      try {
        const items = await bridge.componentUpdateStatus()
        if (installFinished) return
        setComponents(items)
        const next = componentStatus(items)
        if (next.state !== 'available') applyStatus(next)
      } catch {
        // The install command returns the authoritative terminal state.
      } finally {
        pollInFlight = false
      }
    }
    const poll = window.setInterval(() => void refreshProgress(), 250)
    try {
      const items = await bridge.installComponentUpdates()
      installFinished = true
      setComponents(items)
      applyStatus(componentStatus(items))
      if (
        !items.some((item) => item.state === 'error') &&
        items.some((item) => item.restartRequired)
      ) {
        await bridge.restartForComponentUpdate?.()
      }
      return items
    } finally {
      installFinished = true
      window.clearInterval(poll)
    }
  }, [applyStatus, bridge])

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

  const download = useCallback(() => {
    if (mobileModeRef.current && mobileInvoke && status.downloadUrl) {
      return mobileInvoke<boolean>('mobile_open_app_update', { url: status.downloadUrl })
    }
    return openReleases()
  }, [mobileInvoke, openReleases, status.downloadUrl])

  const install = useCallback(() => download(), [download])

  const effectiveInfo = useMemo(
    () => ({
      ...info,
      version: info.desktop
        ? currentDesktopVersion(info.hostVersion || info.version, components)
        : info.version,
    }),
    [components, info],
  )

  return {
    info: effectiveInfo,
    status,
    components,
    check,
    installComponents,
    download,
    install,
    openReleases,
    openUpdateLog,
  }
}
