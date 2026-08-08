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

function componentStatus(items: ComponentUpdateStatus[]): UpdateStatus {
  const checkedAt = new Date().toISOString()
  const failed = items.filter((item) => item.state === 'error')
  const available = items.filter((item) => item.state === 'available')
  const checking = items.some((item) => item.state === 'checking')
  const downloading = items.some((item) => item.state === 'downloading')
  const installed = items.some((item) => item.state === 'installed')
  const release = available[0] || failed[0] || items[0]
  const notes = available
    .map((item) => item.notes.trim())
    .filter((value, index, values) => value && values.indexOf(value) === index)
    .join('\n\n')

  return {
    state: failed.length
      ? 'error'
      : downloading
        ? 'downloading'
        : checking
          ? 'checking'
          : available.length
            ? 'available'
            : installed
              ? 'installed'
              : items.length
                ? 'current'
                : 'idle',
    checkedAt,
    message: failed.map((item) => `${item.component}: ${item.message}`).join('\n'),
    releaseUrl: release?.releaseUrl || RELEASES_URL,
    canDownload: available.length > 0,
    availableVersion: release?.availableVersion,
    notes,
    total: available.reduce((total, item) => total + item.size, 0),
  }
}

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

    bridge
      .getAppInfo()
      .then((value) => {
        if (!active) return
        setInfo(value)
        void bridge
          .componentUpdateStatus?.()
          .then((items) => {
            if (!active) return
            setComponents(items)
            applyStatus(componentStatus(items))
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
    }
  }, [applyStatus, bridge, check])

  const installComponents = useCallback(async () => {
    if (!bridge?.installComponentUpdates) return []
    applyStatus({ ...statusRef.current, state: 'downloading', message: '' })
    const items = await bridge.installComponentUpdates()
    setComponents(items)
    applyStatus(componentStatus(items))
    if (
      !items.some((item) => item.state === 'error') &&
      items.some((item) => item.restartRequired)
    ) {
      await bridge.restartForComponentUpdate?.()
    }
    return items
  }, [applyStatus, bridge])

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
    installComponents,
    download,
    install,
    openReleases,
    openUpdateLog,
  }
}
