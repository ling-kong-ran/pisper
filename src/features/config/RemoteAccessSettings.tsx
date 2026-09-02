// 远程访问设置：开关局域网监听、展示接入地址与证书指纹、
// 生成配对二维码、管理已配对设备（吊销）。数据全部来自 runtime 的 /api/remote/*。
import { useCallback, useEffect, useState } from 'react'
import {
  Check,
  LoaderCircle,
  MonitorSmartphone,
  QrCode,
  RefreshCw,
  Smartphone,
  X,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { SettingsCard as Panel, SettingsSwitch as Switch } from './settings-primitives'
import { Button } from '@/components/ui/button'
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
      const [nextStatus, nextDevices, nextApprovals] = await Promise.all([
        apiJson<RemoteStatus>('/api/remote/status'),
        apiJson<{ devices: RemoteDevice[] }>('/api/remote/devices'),
        apiJson<{ requests: PairingApproval[] }>('/api/remote/pairing-requests').catch(() => ({
          requests: [],
        })),
      ])
      setStatus(nextStatus)
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
      await apiJson('/api/remote/enabled', { method: 'PUT', body: { enabled } })
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

  const generatePairingCode = async () => {
    setBusy(true)
    setPairingError('')
    try {
      const result = await apiJson<PairingCode>('/api/remote/pairing-code', { method: 'POST' })
      if (!result?.code || !result?.expiresAt)
        throw new Error(t('config:remoteAccess.qrUnavailable'))
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

  if (loadError) {
    return <Panel className="p-4 text-[13px] text-[var(--text-muted)]">{loadError}</Panel>
  }
  if (!status) return null

  return (
    <div className="flex flex-col gap-4">
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
                className="size-64 max-w-full rounded-[var(--r-sm)] bg-white p-2"
              />
            ) : null}
            {pairingError ? (
              <p className="text-center text-[12px] leading-relaxed text-[var(--danger)]">
                {pairingError}
              </p>
            ) : null}
            {pairing ? (
              <div className="text-center">
                <div className="font-mono text-[20px] tracking-[0.2em]">{pairing.code}</div>
                <div className="mt-1 text-[11px] text-[var(--text-muted)]">
                  {t('config:remoteAccess.codeExpiresAt', {
                    time: relativeTime(pairing.expiresAt, language),
                  })}
                </div>
              </div>
            ) : null}
          </div>
          <DialogFooter>
            <Button disabled={busy} onClick={() => void generatePairingCode()}>
              {busy ? <LoaderCircle className="animate-spin" /> : <RefreshCw />}
              {busy ? t('config:remoteAccess.generatingQr') : t('config:remoteAccess.regenerate')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Panel className="flex flex-col gap-3 p-4" data-config-card="remote-access-main">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-start gap-[11px]">
            <span className="grid size-[38px] flex-none place-items-center rounded-[var(--r-sm)] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <MonitorSmartphone size={19} />
            </span>
            <div>
              <h2 className="text-[16px]">{t('config:remoteAccess.title')}</h2>
              <p className="mt-1 text-[13px] leading-[1.55] text-[var(--text-muted)]">
                {t('config:remoteAccess.description')}
              </p>
            </div>
          </div>
          <Switch
            value={status.enabled && status.listening}
            disabled={busy}
            onChange={(checked) => void toggleEnabled(checked)}
            ariaLabel={t('config:remoteAccess.title')}
          />
        </div>
        {status.error ? (
          <p className="rounded-[var(--r-sm)] bg-[var(--danger-soft)] px-3 py-2 text-[12px] text-[var(--danger)]">
            {t('config:remoteAccess.listenFailed', { message: status.error })}
          </p>
        ) : null}
      </Panel>

      {status.listening && approvals.length ? (
        <Panel className="flex flex-col gap-3 p-4" aria-live="polite">
          <div className="flex items-center gap-2 text-[14px]">
            <Smartphone size={16} />
            {t('config:remoteAccess.pendingApprovals')}
          </div>
          <p className="text-[12px] leading-relaxed text-[var(--text-muted)]">
            {t('config:remoteAccess.pendingApprovalsDescription')}
          </p>
          <ul className="flex flex-col gap-2">
            {approvals.map((approval) => (
              <li
                key={approval.id}
                className="flex items-center justify-between gap-3 rounded-[var(--r-sm)] border border-[var(--border)] px-3 py-2"
              >
                <div className="min-w-0">
                  <div className="truncate text-[13px]">{approval.deviceName}</div>
                  <div className="text-[11px] text-[var(--text-muted)]">
                    {t('config:remoteAccess.requestedAt', {
                      time: relativeTime(approval.requestedAt, language),
                    })}
                  </div>
                  <code className="block truncate text-[11px] text-[var(--text-muted)]">
                    {t('config:remoteAccess.requestSource', { ip: approval.ip })}
                  </code>
                </div>
                <div className="flex flex-none items-center gap-1">
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
                      <LoaderCircle className="animate-spin" />
                    ) : (
                      <Check />
                    )}
                    {t('config:remoteAccess.approve')}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        </Panel>
      ) : null}

      {status.listening ? (
        <Panel className="flex flex-col gap-3 p-4" data-config-card="remote-access-pairing">
          <div className="flex items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-[14px]">
              <QrCode size={16} />
              {t('config:remoteAccess.pairing')}
            </div>
            <Button size="sm" disabled={busy} onClick={() => void generatePairingCode()}>
              {busy ? <LoaderCircle className="animate-spin" /> : <QrCode />}
              {busy ? t('config:remoteAccess.generatingQr') : t('config:remoteAccess.generateQr')}
            </Button>
          </div>
          <p className="text-[12px] leading-[1.6] text-[var(--text-muted)]">
            {t('config:remoteAccess.pairingDescription')}
          </p>
          {pairingError && !pairingOpen ? (
            <p className="text-[12px] leading-relaxed text-[var(--danger)]" aria-live="polite">
              {pairingError}
            </p>
          ) : null}
        </Panel>
      ) : null}

      <Panel className="flex flex-col gap-3 p-4" data-config-card="remote-access-devices">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-center gap-2 text-[14px]">
            <Smartphone size={16} />
            {t('config:remoteAccess.devices')}
          </div>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void refresh()}
            aria-label={t('config:remoteAccess.refresh')}
          >
            <RefreshCw size={14} />
          </Button>
        </div>
        {devices.length === 0 ? (
          <p className="text-[12px] text-[var(--text-muted)]">
            {t('config:remoteAccess.noDevices')}
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {devices.map((device) => (
              <li
                key={device.id}
                className="flex items-center justify-between gap-3 rounded-[var(--r-sm)] border border-[var(--border)] px-3 py-2"
              >
                <div className="min-w-0">
                  <div className="truncate text-[13px]">
                    {device.name}
                    {device.current ? (
                      <span className="ml-2 text-[11px] text-[var(--text-muted)]">
                        {t('config:remoteAccess.currentDevice')}
                      </span>
                    ) : null}
                  </div>
                  <div className="text-[11px] text-[var(--text-muted)]">
                    {t('config:remoteAccess.lastSeen', {
                      time: relativeTime(device.lastSeenAt, language),
                    })}
                  </div>
                </div>
                <Button size="sm" variant="ghost" onClick={() => void revokeDevice(device)}>
                  {t('config:remoteAccess.revoke')}
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </div>
  )
}
