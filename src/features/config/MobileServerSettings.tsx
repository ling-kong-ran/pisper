// 移动端「服务器」设置页：列出壳内已配对的桌面端，支持配对、切换与删除。
// 数据来自移动端壳（Tauri 命令），配对与令牌管理始终不经过 Runtime。
import { useCallback, useEffect, useState } from 'react'
import {
  AlertCircle,
  CheckCircle2,
  MonitorSmartphone,
  Plus,
  Server,
  Smartphone,
  Trash2,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { SettingsCard as Panel, SettingsCardHeader } from './settings-primitives'
import { Badge } from '@/components/ui/badge'
import { cn } from '@/lib/utils'
import { MobilePairingDialog } from './MobilePairingDialog'
import { Button } from '@/components/ui/button'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

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

export function MobileServerSettings({
  requestConfirm,
}: {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
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

  const forgetServer = async (server: ServerItem) => {
    if (busyId) return
    // WKWebView 不保证提供浏览器 confirm，使用 App 对话框确保取消和确认都可操作。
    const approved = await requestConfirm({
      title: t('config:mobileServer.forget'),
      message: t('config:mobileServer.forgetConfirm'),
      confirmLabel: t('config:mobileServer.forget'),
      tone: 'danger',
    })
    if (!approved) return
    await run('mobile_forget_server', { id: server.id }, server.id)
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
  const remoteActive = state?.mode === 'remote' && Boolean(state.activeId)
  const connected = localActive || remoteActive
  const statusLabel = localActive
    ? t('config:mobileServer.currentLocal')
    : remoteActive
      ? t('config:mobileServer.current')
      : t('config:mobileServer.empty')
  const StatusIcon = connected ? CheckCircle2 : AlertCircle

  return (
    <div className="flex flex-col gap-4">
      <div
        className={cn(
          'flex min-h-11 items-center gap-2 rounded-md border px-3 py-2 text-xs',
          connected
            ? 'border-success-border bg-success-soft text-success-strong'
            : 'border-stroke bg-surface-subtle text-content-muted',
        )}
        role="status"
        aria-live="polite"
      >
        <StatusIcon size={15} aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">{statusLabel}</span>
        {state?.activeTransport ? (
          <span className="flex-none font-mono text-xs opacity-80">
            {state.activeTransport === 'iroh'
              ? t('config:mobileServer.transportP2p')
              : t('config:mobileServer.transportLan')}
          </span>
        ) : null}
      </div>
      <MobilePairingDialog open={pairingOpen} onOpenChange={setPairingOpen} />
      <Panel className="flex flex-col gap-3 p-4" data-config-card="mobile-server-main">
        <SettingsCardHeader
          icon={<MonitorSmartphone size={18} />}
          title={t('config:mobileServer.title')}
          description={t('config:mobileServer.description')}
          action={
            <Button
              size="sm"
              className="pointer-coarse:h-11 max-[650px]:h-11 max-[650px]:px-3"
              onClick={() => setPairingOpen(true)}
            >
              <Plus size={14} />
              {t('config:mobileServer.addServer')}
            </Button>
          }
        />
        {error ? <p className="text-xs text-danger">{error}</p> : null}
      </Panel>

      <Panel className="flex flex-col gap-3 p-4" data-config-card="mobile-server-local">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <SettingsCardHeader
            className="min-w-0 flex-1 basis-64"
            active={localActive}
            icon={<Smartphone size={18} />}
            title={
              <span className="flex flex-wrap items-center gap-2">
                {t('config:mobileServer.localTitle')}
                {localActive ? (
                  <Badge variant="accent">{t('config:mobileServer.currentLocal')}</Badge>
                ) : null}
              </span>
            }
            description={t('config:mobileServer.localDescription')}
          />
          <Button
            size="sm"
            variant="secondary"
            className="flex-none max-[650px]:h-11 max-[650px]:px-3 pointer-coarse:h-11"
            disabled={localActive || Boolean(busyId)}
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
          <ul className="divide-y divide-stroke-soft rounded-md border border-stroke">
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
                  className="flex min-h-12 items-center justify-between gap-3 px-3 py-2 max-[650px]:flex-wrap max-[650px]:items-start"
                >
                  <div className="flex min-w-0 items-center gap-2">
                    <Server size={15} aria-hidden="true" className="flex-none text-content-muted" />
                    <div className="min-w-0">
                      <div className="flex min-w-0 items-center gap-2 text-sm font-medium text-content">
                        <span className="truncate">{server.name}</span>
                        {active ? (
                          <Badge variant="accent">{t('config:mobileServer.current')}</Badge>
                        ) : null}
                      </div>
                      <div className="truncate font-mono text-xs text-content-muted">
                        {endpointLabel}
                        <span className="ml-2 font-sans">
                          {directEndpoint && irohEndpoint
                            ? t('config:mobileServer.transportLanP2p')
                            : irohEndpoint
                              ? t('config:mobileServer.transportP2p')
                              : t('config:mobileServer.transportLan')}
                        </span>
                      </div>
                      {active && state.activeTransport ? (
                        <div className="text-xs text-accent-fg">
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
                        className="pointer-coarse:h-11 max-[650px]:h-11 max-[650px]:px-3"
                        disabled={busyId === server.id}
                        onClick={() => void selectServer(server.id)}
                      >
                        {t('config:mobileServer.connect')}
                      </Button>
                    ) : null}
                    <Button
                      size="sm"
                      variant="ghost"
                      className="hover:bg-danger-soft hover:text-danger pointer-coarse:size-11 max-[650px]:size-11"
                      aria-label={t('config:mobileServer.forget')}
                      title={t('config:mobileServer.forget')}
                      disabled={Boolean(busyId)}
                      onClick={() => void forgetServer(server)}
                    >
                      <Trash2 size={14} />
                    </Button>
                  </div>
                </li>
              )
            })}
          </ul>
        ) : (
          <p className="text-xs text-content-muted">{t('config:mobileServer.empty')}</p>
        )}
      </Panel>
    </div>
  )
}
