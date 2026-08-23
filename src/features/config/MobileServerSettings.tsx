// 移动端「服务器」设置页：列出壳内已配对的桌面端，支持配对、切换与删除。
// 数据来自移动端壳（Tauri 命令），配对与令牌管理始终不经过 Runtime。
import { useCallback, useEffect, useState } from 'react'
import { MonitorSmartphone, Plus, Server, Smartphone, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { SettingsCard as Panel } from './settings-primitives'
import { MobilePairingDialog } from './MobilePairingDialog'
import { Button } from '@/components/ui/button'

type ServerEndpoint = {
  t: string
  url?: string
  nodeId?: string
}

type ServerItem = {
  id: string
  name: string
  endpoints: ServerEndpoint[]
  pairedAt: string
}

type OnDeviceState = {
  supported: boolean
  running: boolean
  state: string
  message: string
  url: string
  runtimeKind: string
}

type MobileState = {
  paired: boolean
  proxyUrl: string
  mode: 'local' | 'remote' | null
  onDevice: OnDeviceState
  activeId: string | null
  activeTransport: string | null
  servers: ServerItem[]
}

function invokeMobile<T>(command: string, args?: unknown): Promise<T> {
  const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
  if (!invoke) return Promise.reject(new Error('native bridge unavailable'))
  return invoke<T>(command, args)
}

export function MobileServerSettings() {
  const { t } = useI18n()
  const [state, setState] = useState<MobileState | null>(null)
  const [error, setError] = useState('')
  const [busyId, setBusyId] = useState('')
  const [pairingOpen, setPairingOpen] = useState(false)

  const refresh = useCallback(async () => {
    try {
      setState(await invokeMobile<MobileState>('mobile_state'))
      setError('')
    } catch {
      setError(t('config:mobileServer.loadFailed'))
    }
  }, [t])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const run = async (command: string, args: unknown, id = '') => {
    setBusyId(id || command)
    try {
      setState(await invokeMobile<MobileState>(command, args))
      setError('')
    } catch (cause) {
      setError(String(cause))
    } finally {
      setBusyId('')
    }
  }

  const selectServer = async (id: string) => {
    setBusyId(id)
    try {
      const updated = await invokeMobile<MobileState>('mobile_select_server', { id })
      setState(updated)
      setError('')
      // 远程模式沿用原代理链路，离开当前本机 Runtime 的页面来源。
      if (updated.proxyUrl) window.location.replace(updated.proxyUrl)
    } catch (cause) {
      setError(String(cause))
    } finally {
      setBusyId('')
    }
  }

  const localActive = state?.mode === 'local'

  return (
    <div className="flex flex-col gap-4">
      <MobilePairingDialog open={pairingOpen} onOpenChange={setPairingOpen} />
      <Panel className="flex flex-col gap-3 p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-start gap-[11px]">
            <span className="grid size-[38px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <MonitorSmartphone size={19} />
            </span>
            <div>
              <h2 className="text-[16px]">{t('config:mobileServer.title')}</h2>
              <p className="mt-1 text-[13px] leading-[1.55] text-[var(--text-muted)]">
                {t('config:mobileServer.description')}
              </p>
            </div>
          </div>
          <Button size="sm" onClick={() => setPairingOpen(true)}>
            <Plus size={14} />
            {t('config:mobileServer.addServer')}
          </Button>
        </div>
        {error ? <p className="text-[12px] text-[var(--danger)]">{error}</p> : null}
      </Panel>

      <Panel className="flex flex-col gap-3 p-4">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-start gap-[11px]">
            <span className="grid size-[38px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <Smartphone size={19} />
            </span>
            <div>
              <h2 className="flex items-center gap-2 text-[16px]">
                {t('config:mobileServer.localTitle')}
                {localActive ? (
                  <span className="text-[11px] text-[var(--star-strong)]">
                    {t('config:mobileServer.currentLocal')}
                  </span>
                ) : null}
              </h2>
              <p className="mt-1 text-[13px] leading-[1.55] text-[var(--text-muted)]">
                {t('config:mobileServer.localDescription')}
              </p>
            </div>
          </div>
          <Button
            size="sm"
            variant="outline"
            disabled={localActive || busyId === 'mobile_enter_local'}
            onClick={() =>
              void (async () => {
                setBusyId('mobile_enter_local')
                try {
                  // 壳只启动同源 Node Runtime，并返回它的认证入口。
                  const updated = await invokeMobile<MobileState>('mobile_enter_local')
                  if (updated.onDevice.url) window.location.replace(updated.onDevice.url)
                } catch (cause) {
                  setError(String(cause))
                } finally {
                  setBusyId('')
                }
              })()
            }
          >
            {localActive
              ? t('config:mobileServer.currentLocal')
              : t('config:mobileServer.enterLocal')}
          </Button>
        </div>
      </Panel>

      <Panel className="flex flex-col gap-3 p-4">
        {state?.servers?.length ? (
          <ul className="flex flex-col gap-2">
            {state.servers.map((server) => {
              const active = state.mode === 'remote' && server.id === state.activeId
              const directEndpoint = server.endpoints.find((endpoint) => endpoint.url)
              const irohEndpoint = server.endpoints.find((endpoint) => endpoint.t === 'iroh')
              const endpointLabel =
                directEndpoint?.url ||
                (irohEndpoint?.nodeId ? `Iroh ${irohEndpoint.nodeId.slice(0, 12)}` : 'Iroh')
              return (
                <li
                  key={server.id}
                  className="flex items-center justify-between gap-3 rounded-[var(--r-sm)] border border-[var(--border)] px-3 py-2"
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <Server size={15} className="flex-none text-[var(--text-muted)]" />
                    <div className="min-w-0">
                      <div className="truncate text-[13px]">
                        {server.name}
                        {active ? (
                          <span className="ml-2 text-[11px] text-[var(--star-strong)]">
                            {t('config:mobileServer.current')}
                          </span>
                        ) : null}
                      </div>
                      <div className="truncate text-[11px] text-[var(--text-muted)]">
                        {endpointLabel}
                        <span className="ml-2">
                          {directEndpoint && irohEndpoint
                            ? t('config:mobileServer.transportLanP2p')
                            : irohEndpoint
                              ? t('config:mobileServer.transportP2p')
                              : t('config:mobileServer.transportLan')}
                        </span>
                      </div>
                      {active && state.activeTransport ? (
                        <div className="text-[11px] text-[var(--star-strong)]">
                          {t('config:mobileServer.activeTransport', {
                            transport:
                              state.activeTransport === 'iroh'
                                ? t('config:mobileServer.transportP2p')
                                : t('config:mobileServer.transportLan'),
                          })}
                        </div>
                      ) : null}
                    </div>
                  </div>
                  <div className="flex flex-none items-center gap-1">
                    {!active ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={busyId === server.id}
                        onClick={() => void selectServer(server.id)}
                      >
                        {t('config:mobileServer.connect')}
                      </Button>
                    ) : null}
                    <Button
                      size="sm"
                      variant="ghost"
                      aria-label={t('config:mobileServer.forget')}
                      onClick={() => {
                        if (window.confirm(t('config:mobileServer.forgetConfirm'))) {
                          void run('mobile_forget_server', { id: server.id }, server.id)
                        }
                      }}
                    >
                      <Trash2 size={14} />
                    </Button>
                  </div>
                </li>
              )
            })}
          </ul>
        ) : (
          <p className="text-[12px] text-[var(--text-muted)]">{t('config:mobileServer.empty')}</p>
        )}
      </Panel>
    </div>
  )
}
