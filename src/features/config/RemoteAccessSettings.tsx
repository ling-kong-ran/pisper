import './remote-workspace-messages'
// 远程访问（功能 › 设备与连接，功能页模板的分区与列表行）：开关局域网监听、系统防火墙状态、
// 待审批的连接、生成配对二维码、管理已配对设备（吊销）。数据全部来自 runtime 的 /api/remote/*。
import { useCallback, useEffect, useState, type ReactNode } from 'react'
import {
  Check,
  MonitorSmartphone,
  QrCode,
  RefreshCw,
  ShieldCheck,
  Smartphone,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
import type { StatusGlyphKind } from '@/components/app/status-glyph-kinds'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'

type RemoteStatus = {
  apiVersion: number
  enabled: boolean
  listening: boolean
  error: string | null
}

type FirewallStatus = {
  state:
    | 'disabled'
    | 'configured'
    | 'not_needed'
    | 'needs_authorization'
    | 'unsupported'
    | 'cancelled'
    | 'error'
    | 'cleanup_pending'
  reason: string | null
  port: number | null
  scope: 'application' | 'program_port' | 'port'
  busy: boolean
  checkedAt: string | null
  lanReachability: 'unverified'
}

type RemoteDevice = {
  id: string
  name: string
  createdAt: string
  lastSeenAt: string
  revokedAt: string | null
  current: boolean
}

type PairingCode = {
  code: string
  expiresAt: string
  qrDataUrl: string
  connectionInfo?: { fingerprint: string; endpoints: { t: string; url: string }[] }
}

type PairingApproval = {
  id: string
  deviceName: string
  ip: string
  requestedAt: string
  expiresAt: string
}

export function RemoteAccessSettings({ notify }: { notify: Notify }) {
  const { t, language } = useI18n()
  const [status, setStatus] = useState<RemoteStatus | null>(null)
  const [firewall, setFirewall] = useState<FirewallStatus | null>(null)
  const [devices, setDevices] = useState<RemoteDevice[]>([])
  const [approvals, setApprovals] = useState<PairingApproval[]>([])
  const [pairing, setPairing] = useState<PairingCode | null>(null)
  const [pairingOpen, setPairingOpen] = useState(false)
  const [pairingError, setPairingError] = useState('')
  const [approvalBusy, setApprovalBusy] = useState('')
  const [busy, setBusy] = useState(false)
  const [loadError, setLoadError] = useState('')

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextDevices, nextApprovals, nextFirewall] = await Promise.all([
        apiJson<RemoteStatus>('/api/remote/status'),
        apiJson<{ devices: RemoteDevice[] }>('/api/remote/devices'),
        apiJson<{ requests: PairingApproval[] }>('/api/remote/pairing-requests').catch(() => ({
          requests: [],
        })),
        apiJson<FirewallStatus>('/api/remote/firewall').catch((error: unknown) => {
          // 远程设备没有本机策略管理权限，保持普通远程访问设置可用。
          if (error && typeof error === 'object' && 'status' in error && error.status === 403)
            return null
          throw error
        }),
      ])
      setStatus(nextStatus)
      setFirewall(nextFirewall)
      setDevices(nextDevices.devices.filter((device) => !device.revokedAt))
      setApprovals(nextApprovals.requests)
      setLoadError('')
    } catch {
      setLoadError(t('config:remoteAccess.loadFailed'))
    }
  }, [t])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    if (!status?.listening) return
    const timer = window.setInterval(() => void refresh(), 2_000)
    return () => window.clearInterval(timer)
  }, [refresh, status?.listening])

  const toggleEnabled = async (enabled: boolean) => {
    setBusy(true)
    try {
      await apiJson('/api/remote/enabled', {
        method: 'PUT',
        body: { enabled, configureFirewall: firewall !== null },
        timeout: 150_000,
      })
      const invoke = window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__?.invoke
      if (invoke) {
        await invoke('desktop_iroh_set_enabled', { enabled }).catch(() => undefined)
      }
      if (!enabled) {
        setPairing(null)
        setPairingOpen(false)
        setPairingError('')
      }
      await refresh()
    } catch (error) {
      notify(t('config:remoteAccess.toggleFailed', { message: String(error) }), 'error')
    } finally {
      setBusy(false)
    }
  }

  const retryFirewall = async () => {
    setBusy(true)
    try {
      await apiJson<FirewallStatus>('/api/remote/firewall/retry', {
        method: 'POST',
        body: {},
        timeout: 150_000,
      })
      await refresh()
    } catch (error) {
      notify(t('config:remoteAccess.firewallRetryFailed', { message: String(error) }), 'error')
    } finally {
      setBusy(false)
    }
  }

  const firewallStates = {
    disabled: t('config:remoteAccess.firewallDisabled'),
    configured: t('config:remoteAccess.firewallConfigured'),
    not_needed: t('config:remoteAccess.firewallNotNeeded'),
    needs_authorization: t('config:remoteAccess.firewallNeedsAuthorization'),
    unsupported: t('config:remoteAccess.firewallUnsupported'),
    cancelled: t('config:remoteAccess.firewallCancelled'),
    error: t('config:remoteAccess.firewallError'),
    cleanup_pending: t('config:remoteAccess.firewallCleanupPending'),
  }
  const firewallReasons: Record<string, string> = {
    packaged_app_required: t('config:remoteAccess.firewallPackagedApp'),
    unsupported_firewall: t('config:remoteAccess.firewallUnsupportedBackend'),
    unsupported_platform: t('config:remoteAccess.firewallUnsupportedBackend'),
    permission_denied: t('config:remoteAccess.firewallPermissionDenied'),
    cancelled: t('config:remoteAccess.firewallCancelled'),
    timeout: t('config:remoteAccess.firewallTimeout'),
    block_all: t('config:remoteAccess.firewallBlockAll'),
    rule_changed: t('config:remoteAccess.firewallRuleChanged'),
    inspection_failed: t('config:remoteAccess.firewallVerificationFailed'),
    verification_failed: t('config:remoteAccess.firewallVerificationFailed'),
    operation_failed: t('config:remoteAccess.firewallVerificationFailed'),
    unsupported: t('config:remoteAccess.firewallUnsupportedBackend'),
  }

  const generatePairingCode = async () => {
    setBusy(true)
    setPairingError('')
    try {
      const result = await apiJson<PairingCode>('/api/remote/pairing-code', { method: 'POST' })
      if (!result?.code || !result?.expiresAt)
        throw new Error(t('config:remoteAccess.qrUnavailable'))
      // 旧 Runtime 可继续使用二维码；新增的手动信息只从本机受保护端点读取。
      result.connectionInfo = await apiJson<PairingCode['connectionInfo']>(
        '/api/remote/connection-info',
      ).catch(() => undefined)
      setPairing(result)
      setPairingOpen(true)
      if (!result.qrDataUrl) setPairingError(t('config:remoteAccess.qrUnavailable'))
    } catch (error) {
      const message = t('config:remoteAccess.pairingFailed', { message: String(error) })
      setPairingError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  const decideApproval = async (approval: PairingApproval, approved: boolean) => {
    setApprovalBusy(approval.id)
    try {
      await apiJson(`/api/remote/pairing-requests/${encodeURIComponent(approval.id)}/decision`, {
        method: 'POST',
        body: { approved },
      })
      await refresh()
    } catch (error) {
      notify(t('config:remoteAccess.approvalFailed', { message: String(error) }), 'error')
    } finally {
      setApprovalBusy('')
    }
  }

  const revokeDevice = async (device: RemoteDevice) => {
    if (!window.confirm(t('config:remoteAccess.revokeConfirm', { name: device.name }))) return
    try {
      await apiJson(`/api/remote/devices/${encodeURIComponent(device.id)}/revoke`, {
        method: 'POST',
      })
      await refresh()
    } catch (error) {
      notify(t('config:remoteAccess.revokeFailed', { message: String(error) }), 'error')
    }
  }

  if (loadError && !status) {
    return (
      <PageState
        kind="error"
        title={t('config:remoteAccess.loadFailed')}
        action={
          <Button variant="secondary" className="max-[650px]:h-11" onClick={() => void refresh()}>
            <RefreshCw />
            {t('config:remoteAccess.refresh')}
          </Button>
        }
      />
    )
  }
  if (!status) return <PageState kind="loading" title={t('config:remoteAccess.loading')} />

  // 监听状态：启动失败要人处理（琥珀）；正在接受连接、已关闭、正在启动都是中性（规格 6.1）。
  const listenState = status.error
    ? 'failed'
    : status.listening
      ? 'on'
      : status.enabled
        ? 'starting'
        : 'off'
  // 防火墙：还没确认放行（需要授权、授权取消、出错、规则未清理）要人回来处理，琥珀；其余中性。
  const firewallAttention =
    firewall !== null &&
    !firewall.busy &&
    ['needs_authorization', 'cancelled', 'error', 'cleanup_pending'].includes(firewall.state)

  return (
    <>
      <Dialog open={pairingOpen && pairing !== null} onOpenChange={setPairingOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t('config:remoteAccess.pairingQrTitle')}</DialogTitle>
            <DialogDescription>{t('config:remoteAccess.scanQrDescription')}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col items-center gap-3 py-2" aria-live="polite">
            {pairing?.qrDataUrl ? (
              <img
                src={pairing.qrDataUrl}
                alt={t('config:remoteAccess.qrAlt')}
                className="size-64 max-w-full rounded-sm border border-stroke bg-white p-2"
              />
            ) : null}
            {pairingError ? (
              <p className="text-center text-xs leading-relaxed text-danger">{pairingError}</p>
            ) : null}
            {pairing ? (
              <div className="text-center">
                <div className="font-mono text-[20px] font-medium tracking-[0.2em] text-content">
                  {pairing.code}
                </div>
                <div className="mt-1 text-xs text-content-muted tabular-nums">
                  {t('config:remoteAccess.codeExpiresAt', {
                    time: relativeTime(pairing.expiresAt, language),
                  })}
                </div>
              </div>
            ) : null}
          </div>
          {pairing?.connectionInfo && (
            <details className="min-w-0 rounded-md border border-stroke px-3 py-2 text-sm">
              <summary className="cursor-pointer text-content-secondary">
                {t('remote-workspace:manualConnection')}
              </summary>
              <div className="mt-2 space-y-2 text-xs text-content-muted">
                <div>{t('remote-workspace:serverAddress')}</div>
                {pairing.connectionInfo.endpoints.map((endpoint) => (
                  <code
                    key={endpoint.url}
                    className="block font-mono break-all text-content select-all"
                  >
                    {endpoint.url}
                  </code>
                ))}
                <div>{t('remote-workspace:fingerprint')}</div>
                <code className="block font-mono break-all text-content select-all">
                  {pairing.connectionInfo.fingerprint}
                </code>
              </div>
            </details>
          )}
          <DialogFooter>
            <Button disabled={busy} onClick={() => void generatePairingCode()}>
              {busy ? <OrbitSpinner className="text-current" /> : <RefreshCw />}
              {busy ? t('config:remoteAccess.generatingQr') : t('config:remoteAccess.regenerate')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 读到之后的刷新失败：写在最上面，已显示的状态保留（原来整页换成一行错误）。 */}
      {loadError && (
        <p
          role="alert"
          className="mt-4 rounded-[var(--r-md)] border border-danger-border bg-danger-soft px-3 py-2 text-xs text-danger"
        >
          {loadError}
        </p>
      )}
      <FeatureSection
        title={t('config:remoteAccess.thisComputer')}
        data-config-card="remote-access-main"
      >
        <FeatureList>
          <FeatureRow
            icon={<MonitorSmartphone />}
            name={t('config:remoteAccess.allowConnections')}
            description={t('config:remoteAccess.description')}
            status={
              <StateText
                attention={listenState === 'failed'}
                glyph={listenState === 'on' ? 'done' : listenState === 'starting' ? 'run' : null}
              >
                {listenState === 'failed'
                  ? t('config:remoteAccess.stateFailed')
                  : listenState === 'on'
                    ? t('config:remoteAccess.stateListening')
                    : listenState === 'starting'
                      ? t('config:remoteAccess.stateStarting')
                      : t('config:remoteAccess.stateOff')}
              </StateText>
            }
            actions={
              <Switch
                checked={status.enabled}
                disabled={busy}
                onCheckedChange={(checked) => void toggleEnabled(checked)}
                aria-label={t('config:remoteAccess.title')}
              />
            }
            detail={
              status.error ? (
                <p className="text-xs leading-relaxed [overflow-wrap:anywhere] text-st-attn">
                  {t('config:remoteAccess.listenFailed', { message: status.error })}
                </p>
              ) : undefined
            }
          />
          {firewall ? (
            <FeatureRow
              aria-live="polite"
              icon={<ShieldCheck />}
              name={t('config:remoteAccess.firewallTitle')}
              description={
                firewall.busy
                  ? t('config:remoteAccess.firewallWorking')
                  : firewallStates[firewall.state]
              }
              status={
                firewall.busy ? (
                  <StateText glyph="run">{t('config:remoteAccess.firewallChecking')}</StateText>
                ) : firewallAttention ? (
                  <StateText attention>{t('config:remoteAccess.firewallNeedsYou')}</StateText>
                ) : undefined
              }
              actions={
                status.enabled || firewall.state === 'cleanup_pending' ? (
                  <Button
                    size="sm"
                    variant="secondary"
                    disabled={busy || firewall.busy}
                    onClick={() => void retryFirewall()}
                  >
                    {busy || firewall.busy ? (
                      <OrbitSpinner className="text-current" />
                    ) : (
                      <RefreshCw />
                    )}
                    {status.enabled
                      ? t('config:remoteAccess.firewallRetry')
                      : t('config:remoteAccess.firewallRetryCleanup')}
                  </Button>
                ) : undefined
              }
              detail={
                <div className="flex flex-col gap-1.5 text-xs leading-relaxed text-content-muted">
                  {firewall.reason && firewallReasons[firewall.reason] ? (
                    <p className="text-content-secondary">{firewallReasons[firewall.reason]}</p>
                  ) : null}
                  {status.enabled ? (
                    <p>
                      {firewall.scope === 'application'
                        ? t('config:remoteAccess.firewallApplicationScope')
                        : firewall.scope === 'program_port'
                          ? t('config:remoteAccess.firewallProgramPortScope', {
                              port: firewall.port ?? '—',
                            })
                          : t('config:remoteAccess.firewallPortScope', {
                              port: firewall.port ?? '—',
                            })}
                    </p>
                  ) : null}
                  <p>{t('config:remoteAccess.firewallReachability')}</p>
                  {firewall.checkedAt ? (
                    <p className="tabular-nums">
                      {t('config:remoteAccess.firewallCheckedAt', {
                        time: relativeTime(firewall.checkedAt, language),
                      })}
                    </p>
                  ) : null}
                </div>
              }
            />
          ) : null}
        </FeatureList>
      </FeatureSection>

      {status.listening && approvals.length ? (
        // 待审批的连接是需要你的事：分区标题旁琥珀计数，行内“拒绝 / 批准”（规格 6.1）。
        <FeatureSection
          aria-live="polite"
          title={t('config:remoteAccess.pendingApprovals')}
          count={approvals.length}
          meta={
            <span className="font-medium text-st-attn">
              {t('config:remoteAccess.waitingForYou')}
            </span>
          }
          description={t('config:remoteAccess.pendingApprovalsDescription')}
        >
          <FeatureList>
            {approvals.map((approval) => (
              <FeatureRow
                key={approval.id}
                data-pairing-request={approval.id}
                icon={<Smartphone />}
                name={approval.deviceName}
                description={
                  <span className="font-mono">
                    {t('config:remoteAccess.requestSource', { ip: approval.ip })}
                  </span>
                }
                meta={t('config:remoteAccess.requestedAt', {
                  time: relativeTime(approval.requestedAt, language),
                })}
                actions={
                  <>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={approvalBusy === approval.id}
                      onClick={() => void decideApproval(approval, false)}
                    >
                      <X />
                      {t('config:remoteAccess.reject')}
                    </Button>
                    <Button
                      size="sm"
                      disabled={approvalBusy === approval.id}
                      onClick={() => void decideApproval(approval, true)}
                    >
                      {approvalBusy === approval.id ? (
                        <OrbitSpinner className="text-current" />
                      ) : (
                        <Check />
                      )}
                      {t('config:remoteAccess.approve')}
                    </Button>
                  </>
                }
              />
            ))}
          </FeatureList>
        </FeatureSection>
      ) : null}

      {status.listening ? (
        <FeatureSection
          title={t('config:remoteAccess.pairing')}
          data-config-card="remote-access-pairing"
        >
          <FeatureList>
            <FeatureRow
              icon={<QrCode />}
              name={t('config:remoteAccess.pairWithPhone')}
              description={t('config:remoteAccess.pairingDescription')}
              actions={
                <Button size="sm" disabled={busy} onClick={() => void generatePairingCode()}>
                  {busy ? <OrbitSpinner className="text-current" /> : <QrCode />}
                  {busy
                    ? t('config:remoteAccess.generatingQr')
                    : t('config:remoteAccess.generateQr')}
                </Button>
              }
              detail={
                pairingError && !pairingOpen ? (
                  <p className="text-xs leading-relaxed text-danger" aria-live="polite">
                    {pairingError}
                  </p>
                ) : undefined
              }
            />
          </FeatureList>
        </FeatureSection>
      ) : null}

      <FeatureSection
        title={t('config:remoteAccess.devices')}
        count={devices.length}
        data-config-card="remote-access-devices"
        actions={
          <Button
            size="icon-sm"
            variant="ghost"
            className="pointer-coarse:size-11 max-[650px]:size-11"
            onClick={() => void refresh()}
            aria-label={t('config:remoteAccess.refresh')}
            title={t('config:remoteAccess.refresh')}
          >
            <RefreshCw />
          </Button>
        }
      >
        {devices.length === 0 ? (
          <InlineEmptyState compact title={t('config:remoteAccess.noDevices')} />
        ) : (
          <FeatureList>
            {devices.map((device) => (
              <FeatureRow
                key={device.id}
                icon={<Smartphone />}
                name={device.name}
                nameMeta={device.current ? t('config:remoteAccess.currentDevice') : undefined}
                description={t('config:remoteAccess.lastSeen', {
                  time: relativeTime(device.lastSeenAt, language),
                })}
                actions={
                  <Button
                    size="sm"
                    variant="ghost"
                    className="hover:bg-danger-soft hover:text-danger"
                    aria-label={`${t('config:remoteAccess.revoke')}: ${device.name}`}
                    onClick={() => void revokeDevice(device)}
                  >
                    {t('config:remoteAccess.revoke')}
                  </Button>
                }
              />
            ))}
          </FeatureList>
        )}
      </FeatureSection>
    </>
  )
}

// 行右侧的状态文字：字形 + 文字；需要你时是琥珀圆点 + 琥珀文字，其余中性（规格 6.1、6.5）。
function StateText({
  attention = false,
  glyph = null,
  children,
}: {
  attention?: boolean
  glyph?: StatusGlyphKind | null
  children: ReactNode
}) {
  const kind = attention ? 'attention' : glyph
  return (
    <span
      className={cn(
        'inline-flex min-w-0 items-center gap-1.5 whitespace-nowrap',
        attention ? 'font-medium text-st-attn' : 'text-content-secondary',
      )}
    >
      {kind && <StatusGlyph status={kind} className="size-3.5" />}
      {children}
    </span>
  )
}
