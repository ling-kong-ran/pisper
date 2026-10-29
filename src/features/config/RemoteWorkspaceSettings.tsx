import './remote-workspace-messages'
import { useCallback, useEffect, useState } from 'react'
import { ArrowUpRight, RefreshCw, Server, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { RemoteWorkspaceSummary } from '@/types/remote-workspace'
import { SettingsCard, SettingsField } from './settings-primitives'

export function RemoteWorkspaceSettings({
  requestConfirm,
}: {
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const bridge = window.pisperDesktop?.remoteWorkspaces
  const [servers, setServers] = useState<RemoteWorkspaceSummary[]>([])
  const [name, setName] = useState('')
  const [address, setAddress] = useState('')
  const [fingerprint, setFingerprint] = useState('')
  const [code, setCode] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const refresh = useCallback(async () => {
    if (bridge) setServers(await bridge.list())
  }, [bridge])

  useEffect(() => {
    void refresh().catch((cause: unknown) => setError(String(cause)))
  }, [refresh])

  const run = async (operation: () => Promise<void>) => {
    if (busy) return
    setBusy(true)
    setError('')
    try {
      await operation()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusy(false)
    }
  }

  if (window.__PISPER_REMOTE_WORKSPACE__) {
    return (
      <SettingsCard data-config-card="remote-workspaces" className="space-y-3">
        <h2 className="flex items-center gap-2 text-[15px]! leading-6 font-medium text-content">
          <Server className="size-4 text-content-muted" aria-hidden="true" />
          {t('remote-workspace:activeTitle')}
        </h2>
        <p className="text-sm leading-relaxed text-content-secondary">
          {t('remote-workspace:activeDescription')}
        </p>
        <p className="text-xs leading-relaxed text-content-muted">
          {t('remote-workspace:isolation')}
        </p>
      </SettingsCard>
    )
  }
  if (!bridge) return null

  return (
    <SettingsCard
      data-config-card="remote-workspaces"
      className="@container/card space-y-4 p-4 py-4"
    >
      <header className="flex items-start justify-between gap-3">
        <div className="flex min-w-0 items-start gap-3">
          <span className="grid size-9.5 flex-none place-items-center rounded-lg border border-key-border bg-key text-content-muted shadow-key">
            <Server size={18} aria-hidden="true" />
          </span>
          <div className="min-w-0">
            <h2 className="text-[15px]! leading-6 font-medium text-content">
              {t('config:remoteWorkspace.title')}
            </h2>
            <p className="mt-0.5 max-w-[64ch] text-xs leading-relaxed text-content-muted">
              {t('remote-workspace:description')}
            </p>
          </div>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="pointer-coarse:size-11"
          disabled={busy}
          aria-label={t('remote-workspace:refresh')}
          onClick={() => void run(refresh)}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </header>
      <p className="rounded-sm bg-surface-subtle px-3 py-2 text-xs leading-relaxed text-content-secondary">
        {t('remote-workspace:instructions')}
      </p>
      {servers.length > 0 && (
        <ul className="divide-y divide-stroke-soft rounded-md border border-stroke">
          {servers.map((server) => (
            <li key={server.id} className="flex min-h-12 items-center gap-3 px-3 py-2">
              <Server className="size-4 shrink-0 text-content-muted" aria-hidden="true" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium text-content">{server.name}</div>
                <div className="truncate font-mono text-xs text-content-muted">
                  {server.address}
                </div>
              </div>
              <Button
                type="button"
                size="sm"
                variant="secondary"
                disabled={busy}
                onClick={() =>
                  void run(async () => {
                    await bridge.open(server.id)
                    await refresh()
                  })
                }
              >
                {server.connected ? t('remote-workspace:show') : t('remote-workspace:open')}
                <ArrowUpRight className="size-3.5" />
              </Button>
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                className="hover:bg-danger-soft hover:text-danger pointer-coarse:size-11"
                disabled={busy}
                aria-label={t('remote-workspace:forget', { name: server.name })}
                onClick={() =>
                  void run(async () => {
                    if (
                      await requestConfirm({
                        title: t('remote-workspace:forget', { name: server.name }),
                        message: t('remote-workspace:forgetDescription'),
                      })
                    ) {
                      await bridge.forget(server.id)
                      await refresh()
                    }
                  })
                }
              >
                <Trash2 className="size-3.5" />
              </Button>
            </li>
          ))}
        </ul>
      )}
      <form
        className="divide-y divide-stroke-soft border-y border-stroke-soft"
        onSubmit={(event) => {
          event.preventDefault()
          void run(async () => {
            const id = await bridge.pair({ name, address, fingerprint, code })
            setCode('')
            await refresh()
            await bridge.open(id)
            await refresh()
          })
        }}
      >
        <SettingsField label={t('remote-workspace:name')} htmlFor="remote-workspace-name">
          <Input
            id="remote-workspace-name"
            value={name}
            maxLength={160}
            disabled={busy}
            placeholder="Linux"
            onChange={(event) => setName(event.target.value)}
          />
        </SettingsField>
        <SettingsField label={t('remote-workspace:address')} htmlFor="remote-workspace-address">
          <Input
            id="remote-workspace-address"
            type="url"
            value={address}
            required
            disabled={busy}
            placeholder="https://192.168.1.10:5174"
            autoComplete="off"
            spellCheck={false}
            className="font-mono"
            onChange={(event) => setAddress(event.target.value)}
          />
        </SettingsField>
        <SettingsField
          label={t('remote-workspace:fingerprint')}
          htmlFor="remote-workspace-fingerprint"
        >
          <Input
            id="remote-workspace-fingerprint"
            value={fingerprint}
            required
            disabled={busy}
            placeholder="SHA256:…"
            autoComplete="off"
            spellCheck={false}
            className="font-mono text-xs"
            onChange={(event) => setFingerprint(event.target.value)}
          />
        </SettingsField>
        <SettingsField label={t('remote-workspace:code')} htmlFor="remote-workspace-code">
          <Input
            id="remote-workspace-code"
            value={code}
            required
            maxLength={64}
            disabled={busy}
            placeholder="ABCD-EFGH"
            autoComplete="off"
            spellCheck={false}
            className="font-mono"
            onChange={(event) => setCode(event.target.value)}
          />
        </SettingsField>
        <div className="flex justify-end py-3">
          <Button type="submit" className="max-[650px]:h-11" disabled={busy}>
            {busy ? (
              <OrbitSpinner className="text-current" />
            ) : (
              <ArrowUpRight className="size-3.5" />
            )}
            {t('remote-workspace:pair')}
          </Button>
        </div>
      </form>
      {error && (
        <p
          role="alert"
          className="rounded-sm bg-danger-soft px-2.5 py-2 text-sm break-words text-danger"
        >
          {error}
        </p>
      )}
      <p className="text-xs leading-relaxed text-content-muted">
        {t('remote-workspace:isolation')}
      </p>
    </SettingsCard>
  )
}
