import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  AppUpdateController,
  AppUpdateInfo,
  ComponentUpdateStatus,
  UpdateStatus,
} from '@/types/update'
import { scheduleDesktopUpdateChecks, shouldAutomaticallyCheckForUpdates } from './auto-update'
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

export function useAppUpdate(): AppUpdateController {
  const bridge = window.pisperDesktop
  const [info, setInfo] = useState(WEB_INFO)
  const [status, setStatus] = useState<UpdateStatus>({ state: 'idle', checkedAt: null })
  const [components, setComponents] = useState<ComponentUpdateStatus[]>([])
  const statusRef = useRef(status)
  const checkInFlightRef = useRef<Promise<UpdateStatus> | null>(null)

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
          const next: UpdateStatus = bridge
            ? await bridge.checkForUpdates()
            : await checkWebUpdates({ refresh })
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
    [applyStatus, bridge],
  )

  useEffect(() => {
    let active = true
    let stopAutomaticChecks = () => {}
    if (!bridge) {
      void check({ refresh: false })
      return () => {
        active = false
      }
    }

    const unsubscribe = bridge.onUpdateStatus((value) => {
      if (active) applyStatus(value)
    })
    bridge
      .getAppInfo()
      .then((value) => {
        if (!active) return
        setInfo(value)
        applyStatus(value.update || { state: 'idle', checkedAt: null })
        void bridge
          .componentUpdateStatus?.()
          .then((items) => {
            if (active) setComponents(items)
          })
          .catch(() => {})
        if (value.packaged) {
          stopAutomaticChecks = scheduleDesktopUpdateChecks(() => {
            if (!shouldAutomaticallyCheckForUpdates(statusRef.current.state)) return
            return check()
          })
        }
      })
      .catch(() => {})
    return () => {
      active = false
      stopAutomaticChecks()
      unsubscribe?.()
    }
  }, [applyStatus, bridge, check])

  const checkComponents = useCallback(async () => {
    if (!bridge?.checkComponentUpdates) return []
    const items = await bridge.checkComponentUpdates()
    setComponents(items)
    return items
  }, [bridge])

  const installComponent = useCallback(
    async (component: 'tui' | 'runtime') => {
      if (!bridge?.installComponentUpdate) return []
      const items = await bridge.installComponentUpdate(component)
      setComponents(items)
      return items
    },
    [bridge],
  )

  const restartForComponents = useCallback(
    () => bridge?.restartForComponentUpdate?.() || Promise.resolve(false),
    [bridge],
  )

  const openReleases = useCallback(async () => {
    if (bridge) return bridge.openReleases()
    window.open(status.releaseUrl || RELEASES_URL, '_blank', 'noopener,noreferrer')
    return true
  }, [bridge, status.releaseUrl])

  const openUpdateLog = useCallback(() => bridge?.openUpdateLog?.(), [bridge])

  const download = useCallback(async () => {
    if (!bridge || !status.canDownload) return openReleases()
    const next = await bridge.downloadUpdate()
    applyStatus(next)
    return next
  }, [applyStatus, bridge, openReleases, status.canDownload])

  const install = useCallback(() => bridge?.installUpdate(), [bridge])

  return {
    info,
    status,
    components,
    check,
    checkComponents,
    installComponent,
    restartForComponents,
    download,
    install,
    openReleases,
    openUpdateLog,
  }
}
