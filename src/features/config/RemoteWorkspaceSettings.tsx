import './remote-workspace-messages'
// 远程工作区（远程访问页里的一个分区，桌面桥接才有）：已保存的 Linux 工作区各一行（连接 / 显示窗口、移除），
// 下面是配对表单。远程工作区窗口里只显示“当前为远程工作区”的说明，不管理连接。
import { useCallback, useEffect, useState } from 'react'
import { ArrowUpRight, RefreshCw, Server, Trash2 } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'
import type { RemoteWorkspaceSummary } from '@/types/remote-workspace'
import { FeatureAlert, FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { SettingsField } from './settings-primitives'

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
    // 远程工作区窗口里不管理连接：只说明当前所在，以及隔离规则。
    return (
      <FeatureSection
        title={t('config:remoteWorkspace.title')}
        data-config-card="remote-workspaces"
      >
        <FeatureList>
          <FeatureRow
            icon={<Server />}
            name={t('remote-workspace:activeTitle')}
            description={t('remote-workspace:activeDescription')}
            detail={
              <p className="text-xs leading-relaxed text-content-muted">
                {t('remote-workspace:isolation')}
              </p>
            }
          />
        </FeatureList>
      </FeatureSection>
    )
  }
  if (!bridge) return null

  return (
    <FeatureSection
      title={t('config:remoteWorkspace.title')}
      count={servers.length}
      data-config-card="remote-workspaces"
      description={t('remote-workspace:description')}
      actions={
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="pointer-coarse:size-11"
          disabled={busy}
          aria-label={t('remote-workspace:refresh')}
          title={t('remote-workspace:refresh')}
          onClick={() => void run(refresh)}
        >
          <RefreshCw className="size-3.5" />
        </Button>
      }
    >
      {servers.length > 0 && (
        <FeatureList>
          {servers.map((server) => (
            <FeatureRow
              key={server.id}
              icon={<Server />}
              name={server.name}
              description={<span className="font-mono">{server.address}</span>}
              actions={
                <>
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
                    className="hover:bg-danger-soft hover:text-danger max-[650px]:size-11 pointer-coarse:size-11"
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
                </>
              }
            />
          ))}
        </FeatureList>
      )}
      {/* 配对表单：说明 + 四个字段 + “配对并打开”；失败时保留已填内容，错误写在下面（role=alert）。 */}
      <div className="@container/card mt-3 rounded-[var(--r-md)] border border-stroke px-3 pt-2 pb-1">
        <p className="py-1.5 text-xs leading-relaxed text-content-secondary">
          {t('remote-workspace:instructions')}
        </p>
        <form
          className="divide-y divide-stroke-soft border-t border-stroke-soft"
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
      </div>
      {error && <FeatureAlert className="mt-3">{error}</FeatureAlert>}
      <p className="mt-3 text-xs leading-relaxed text-content-muted">
        {t('remote-workspace:isolation')}
      </p>
    </FeatureSection>
  )
}
