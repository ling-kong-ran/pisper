// 手机服务器（功能 › 设备与连接，手机 App 才有；功能页模板的分区与列表行）：当前连接、本机运行，
// 以及壳内已配对的桌面端（连接、删除、添加服务器）。
// 数据来自移动端壳（Tauri 命令），配对与令牌管理始终不经过 Runtime。
import { useCallback, useEffect, useState } from 'react'
import { AlertCircle, Plus, Server, Smartphone, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { InlineEmptyState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
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
  const transportLabel = (transport: string) =>
    transport === 'iroh'
      ? t('config:mobileServer.transportP2p')
      : t('config:mobileServer.transportLan')

  return (
    <>
      {/* 当前连接：中性一行（原来连上时是绿色条，规格 6.1 红绿不表示状态）。 */}
      <div
        className="mt-5 flex min-h-11 items-center gap-2 rounded-[var(--r-md)] border border-stroke bg-surface-subtle px-3 py-2 text-xs text-content-secondary"
        role="status"
        aria-live="polite"
      >
        {connected ? (
          <StatusGlyph status="done" className="size-3.5" />
        ) : (
          <AlertCircle className="size-3.5 flex-none text-content-muted" aria-hidden="true" />
        )}
        <span className="min-w-0 flex-1 truncate">{statusLabel}</span>
        {state?.activeTransport ? (
          <span className="flex-none font-mono text-xs text-content-muted">
            {transportLabel(state.activeTransport)}
          </span>
        ) : null}
      </div>
      <MobilePairingDialog open={pairingOpen} onOpenChange={setPairingOpen} />
      {error ? <FeatureAlert className="mt-3">{error}</FeatureAlert> : null}

      <FeatureSection
        title={t('config:mobileServer.localTitle')}
        data-config-card="mobile-server-local"
      >
        <FeatureList>
          <FeatureRow
            icon={<Smartphone />}
            name={t('config:mobileServer.localTitle')}
            description={t('config:mobileServer.localDescription')}
            status={
              localActive ? (
                <span className="inline-flex items-center gap-1 text-content-secondary">
                  <StatusGlyph status="done" className="size-3.5" />
                  {t('config:mobileServer.currentLocal')}
                </span>
              ) : undefined
            }
            actions={
              localActive ? undefined : (
                <Button
                  size="sm"
                  variant="secondary"
                  className="flex-none max-[650px]:h-11 max-[650px]:px-3 pointer-coarse:h-11"
                  disabled={Boolean(busyId)}
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
                  {t('config:mobileServer.enterLocal')}
                </Button>
              )
            }
          />
        </FeatureList>
      </FeatureSection>

      <FeatureSection
        title={t('config:mobileServer.title')}
        count={state?.servers?.length ?? 0}
        data-config-card="mobile-server-main"
        description={t('config:mobileServer.description')}
        actions={
          <Button size="sm" onClick={() => setPairingOpen(true)}>
            <Plus />
            {t('config:mobileServer.addServer')}
          </Button>
        }
      >
        {state?.servers?.length ? (
          <FeatureList>
            {state.servers.map((server) => {
              const active = state.mode === 'remote' && server.id === state.activeId
              const directEndpoint = server.endpoints.find((endpoint) => endpoint.url)
              const irohEndpoint = server.endpoints.find((endpoint) => endpoint.t === 'iroh')
              const endpointLabel =
                directEndpoint?.url ||
                (irohEndpoint?.nodeId ? `Iroh ${irohEndpoint.nodeId.slice(0, 12)}` : 'Iroh')
              return (
                <FeatureRow
                  key={server.id}
                  data-mobile-server={server.id}
                  icon={<Server />}
                  name={server.name}
                  nameMeta={
                    directEndpoint && irohEndpoint
                      ? t('config:mobileServer.transportLanP2p')
                      : irohEndpoint
                        ? t('config:mobileServer.transportP2p')
                        : t('config:mobileServer.transportLan')
                  }
                  description={<span className="font-mono">{endpointLabel}</span>}
                  status={
                    active ? (
                      <span className="inline-flex items-center gap-1 text-content-secondary">
                        <StatusGlyph status="done" className="size-3.5" />
                        {state.activeTransport
                          ? t('config:mobileServer.activeTransport', {
                              transport: transportLabel(state.activeTransport),
                            })
                          : t('config:mobileServer.current')}
                      </span>
                    ) : undefined
                  }
                  actions={
                    <>
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
                        size="icon-sm"
                        variant="ghost"
                        className="hover:bg-danger-soft hover:text-danger max-[650px]:size-11 pointer-coarse:size-11"
                        aria-label={t('config:mobileServer.forget')}
                        title={t('config:mobileServer.forget')}
                        disabled={Boolean(busyId)}
                        onClick={() => void forgetServer(server)}
                      >
                        <Trash2 />
                      </Button>
                    </>
                  }
                />
              )
            })}
          </FeatureList>
        ) : (
          <InlineEmptyState compact title={t('config:mobileServer.empty')} />
        )}
      </FeatureSection>
    </>
  )
}
