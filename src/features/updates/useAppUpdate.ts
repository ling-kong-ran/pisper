import { useCallback, useEffect, useState } from 'react'
import type { AppUpdateController, AppUpdateInfo, UpdateStatus } from '@/types/update'
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
  const bridge = window.vesperDesktop
  const [info, setInfo] = useState(WEB_INFO)
  const [status, setStatus] = useState<UpdateStatus>({ state: 'idle', checkedAt: null })

  const check = useCallback(
    async ({ refresh = true }: { refresh?: boolean } = {}) => {
      setStatus((current) => ({ ...current, state: 'checking', message: '' }))
      try {
        const next: UpdateStatus = bridge
          ? await bridge.checkForUpdates()
          : await checkWebUpdates({ refresh })
        setStatus(next)
        return next
      } catch (error) {
        const failed = {
          state: 'error',
          message: error instanceof Error ? error.message : String(error),
          checkedAt: new Date().toISOString(),
        }
        setStatus(failed)
        return failed
      }
    },
    [bridge],
  )

  useEffect(() => {
    let active = true
    if (!bridge) {
      void check({ refresh: false })
      return () => {
        active = false
      }
    }

    const unsubscribe = bridge.onUpdateStatus((value) => {
      if (active) setStatus(value)
    })
    bridge
      .getAppInfo()
      .then((value) => {
        if (!active) return
        setInfo(value)
        setStatus(value.update || { state: 'idle', checkedAt: null })
      })
      .catch(() => {})
    return () => {
      active = false
      unsubscribe?.()
    }
  }, [bridge, check])

  const openReleases = useCallback(async () => {
    if (bridge) return bridge.openReleases()
    window.open(status.releaseUrl || RELEASES_URL, '_blank', 'noopener,noreferrer')
    return true
  }, [bridge, status.releaseUrl])

  const openUpdateLog = useCallback(() => bridge?.openUpdateLog?.(), [bridge])

  const download = useCallback(async () => {
    if (!bridge || !status.canDownload) return openReleases()
    const next = await bridge.downloadUpdate()
    setStatus(next)
    return next
  }, [bridge, openReleases, status.canDownload])

  const install = useCallback(() => bridge?.installUpdate(), [bridge])

  return { info, status, check, download, install, openReleases, openUpdateLog }
}
