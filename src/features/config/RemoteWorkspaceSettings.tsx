import './remote-workspace-messages'
import { useCallback, useEffect, useState } from 'react'
import { ArrowUpRight, LoaderCircle, RefreshCw, Server, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { RemoteWorkspaceSummary } from '@/types/remote-workspace'
import { SettingsCard } from './settings-primitives'

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
        <h2 className="flex items-center gap-2 font-semibold">
          <Server className="size-4" />
          {t('remote-workspace:activeTitle')}
        </h2>
        <p className="text-sm leading-relaxed text-muted-foreground">
          {t('remote-workspace:activeDescription')}
        </p>
        <p className="text-xs leading-relaxed text-muted-foreground">
          {t('remote-workspace:isolation')}
        </p>
      </SettingsCard>
    )
  }
  if (!bridge) return null

  return (
    <SettingsCard data-config-card="remote-workspaces" className="space-y-4">
      <header className="flex items-start justify-between gap-3">
        <div className="space-y-1">
          <h2 className="flex items-center gap-2 font-semibold">
            <Server className="size-4" />
            {t('config:remoteWorkspace.title')}
          </h2>
          <p className="max-w-[64ch] text-sm leading-relaxed text-muted-foreground">
            {t('remote-workspace:description')}
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          disabled={busy}
          aria-label={t('remote-workspace:refresh')}
          onClick={() => void run(refresh)}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      </header>
      <p className="rounded-md bg-muted/50 px-3 py-2 text-sm leading-relaxed text-muted-foreground">
        {t('remote-workspace:instructions')}
      </p>
      {servers.length > 0 && (
        <ul className="divide-y rounded-lg border">
          {servers.map((server) => (
            <li key={server.id} className="flex items-center gap-3 px-3 py-2.5">
              <Server className="size-4 shrink-0 text-muted-foreground" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">{server.name}</div>
                <div className="truncate text-xs text-muted-foreground">{server.address}</div>
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
        className="grid gap-3 sm:grid-cols-2"
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
        <div className="space-y-1.5">
          <Label htmlFor="remote-workspace-name">{t('remote-workspace:name')}</Label>
          <Input
            id="remote-workspace-name"
            value={name}
            maxLength={160}
            disabled={busy}
            placeholder="Linux"
            onChange={(event) => setName(event.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="remote-workspace-address">{t('remote-workspace:address')}</Label>
          <Input
            id="remote-workspace-address"
            type="url"
            value={address}
            required
            disabled={busy}
            placeholder="https://192.168.1.10:5174"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setAddress(event.target.value)}
          />
        </div>
        <div className="space-y-1.5 sm:col-span-2">
          <Label htmlFor="remote-workspace-fingerprint">{t('remote-workspace:fingerprint')}</Label>
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
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="remote-workspace-code">{t('remote-workspace:code')}</Label>
          <Input
            id="remote-workspace-code"
            value={code}
            required
            maxLength={64}
            disabled={busy}
            placeholder="ABCD-EFGH"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setCode(event.target.value)}
          />
        </div>
        <div className="flex items-end sm:justify-end">
          <Button type="submit" size="sm" disabled={busy}>
            {busy ? (
              <LoaderCircle className="size-3.5 animate-spin" />
            ) : (
              <ArrowUpRight className="size-3.5" />
            )}
            {t('remote-workspace:pair')}
          </Button>
        </div>
      </form>
      {error && (
        <p role="alert" className="break-words text-sm text-destructive">
          {error}
        </p>
      )}
      <p className="text-xs leading-relaxed text-muted-foreground">
        {t('remote-workspace:isolation')}
      </p>
    </SettingsCard>
  )
}
