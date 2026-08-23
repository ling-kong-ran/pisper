import { useCallback, useEffect, useState } from 'react'
import { Camera, ContactRound, ExternalLink, MapPin, Settings } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { SettingsCard as Panel } from './settings-primitives'

type Capability = 'contacts' | 'camera' | 'location' | 'externalApps'
type PermissionState = 'granted' | 'denied' | 'prompt' | 'unsupported' | 'not-required'

type DeviceState = {
  enabled: Record<Capability, boolean>
  permissions: Record<Capability, PermissionState>
}

const ITEMS = [
  { id: 'contacts', icon: ContactRound },
  { id: 'camera', icon: Camera },
  { id: 'location', icon: MapPin },
  { id: 'externalApps', icon: ExternalLink },
] as const

type Translate = ReturnType<typeof useI18n>['t']

function capabilityLabel(t: Translate, capability: Capability) {
  if (capability === 'contacts') return t('config:mobileDevice.contacts')
  if (capability === 'camera') return t('config:mobileDevice.camera')
  if (capability === 'location') return t('config:mobileDevice.location')
  return t('config:mobileDevice.externalApps')
}

function permissionLabel(t: Translate, permission: PermissionState) {
  if (permission === 'granted') return t('config:mobileDevice.permission.granted')
  if (permission === 'denied') return t('config:mobileDevice.permission.denied')
  if (permission === 'unsupported') return t('config:mobileDevice.permission.unsupported')
  if (permission === 'not-required') return t('config:mobileDevice.permission.notRequired')
  return t('config:mobileDevice.permission.prompt')
}

function invokeMobile<T>(command: string, args?: unknown): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command, args)
}

export function MobileDeviceSettings() {
  const { t } = useI18n()
  const [state, setState] = useState<DeviceState | null>(null)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')

  const refresh = useCallback(async () => {
    try {
      setState(await invokeMobile<DeviceState>('mobile_get_device_capabilities'))
      setError('')
    } catch (cause) {
      setError(String(cause))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const setEnabled = async (capability: Capability, enabled: boolean) => {
    setBusy(capability)
    try {
      setState(
        await invokeMobile<DeviceState>('mobile_set_device_capability', {
          capability,
          enabled,
        }),
      )
      setError('')
    } catch (cause) {
      setError(String(cause))
    } finally {
      setBusy('')
    }
  }

  const requestPermission = async (capability: Capability) => {
    setBusy(capability)
    try {
      await invokeMobile('mobile_request_device_permission', { capability })
      await refresh()
    } catch (cause) {
      setError(String(cause))
    } finally {
      setBusy('')
    }
  }

  return (
    <Panel className="flex flex-col gap-3 p-4">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h2 className="text-[16px]">{t('config:mobileDevice.title')}</h2>
          <p className="mt-1 text-[13px] text-[var(--text-muted)]">
            {t('config:mobileDevice.description')}
          </p>
        </div>
        <Tooltip>
          <TooltipTrigger asChild>
            <Button
              size="sm"
              variant="outline"
              className="flex-none"
              onClick={() =>
                void invokeMobile('mobile_open_device_settings').catch((cause) =>
                  setError(String(cause)),
                )
              }
            >
              <Settings size={15} />
              {t('config:mobileDevice.openSettings')}
            </Button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            {t('config:mobileDevice.openSettingsDescription')}
          </TooltipContent>
        </Tooltip>
      </div>
      {error ? <p className="text-[12px] text-[var(--danger)]">{error}</p> : null}
      <ul className="divide-y divide-[var(--border)]">
        {ITEMS.map(({ id, icon: Icon }) => {
          const permission = state?.permissions[id] ?? 'prompt'
          const enabled = state?.enabled[id] ?? false
          return (
            <li key={id} className="flex min-h-14 items-center justify-between gap-3 py-2">
              <div className="flex min-w-0 items-center gap-3">
                <Icon size={17} className="flex-none text-[var(--text-muted)]" />
                <div className="min-w-0">
                  <div className="text-[13px]">{capabilityLabel(t, id)}</div>
                  <div className="text-[11px] text-[var(--text-muted)]">
                    {permissionLabel(t, permission)}
                  </div>
                </div>
              </div>
              <div className="flex flex-none items-center gap-2">
                {enabled && permission === 'prompt' ? (
                  <Button
                    size="sm"
                    variant="ghost"
                    disabled={busy === id}
                    onClick={() => void requestPermission(id)}
                  >
                    {t('config:mobileDevice.request')}
                  </Button>
                ) : null}
                <Switch
                  checked={enabled}
                  disabled={!state || busy === id || permission === 'unsupported'}
                  aria-label={capabilityLabel(t, id)}
                  onCheckedChange={(checked) => void setEnabled(id, checked)}
                />
              </div>
            </li>
          )
        })}
      </ul>
    </Panel>
  )
}
