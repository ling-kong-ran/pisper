// CLI 安装/修复设置：检测系统 pisper CLI 状态，提供安装/卸载/修复。
import { CheckCircle2, Download, Terminal, Trash2, Wrench } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { SettingsBadge as Badge, SettingsCard as Panel } from './settings-primitives'
import type { Notify } from '@/app/route-context'
import type { DesktopCliStatus } from '@/types/update'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'

import { AppError, AppNotice } from '@/components/ui/app-primitives'

export function CliSettings({ notify }: { notify: Notify }) {
  const { t } = useI18n()
  const bridge = window.pisperDesktop
  const [status, setStatus] = useState<DesktopCliStatus | null>(null)
  const [busy, setBusy] = useState<'install' | 'uninstall' | ''>('')
  const [error, setError] = useState('')

  useEffect(() => {
    if (!bridge?.getCliStatus) return
    let active = true
    bridge
      .getCliStatus()
      .then((next) => active && setStatus(next))
      .catch(
        (caught: unknown) =>
          active && setError(caught instanceof Error ? caught.message : String(caught)),
      )
    return () => {
      active = false
    }
  }, [bridge])

  const supported = Boolean(bridge?.getCliStatus && bridge.installCli && bridge.uninstallCli)

  const install = async () => {
    if (!bridge?.installCli) return
    setBusy('install')
    setError('')
    try {
      const next = await bridge.installCli()
      setStatus(next)
      notify(t('config:cliSettings.installedNotification'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const uninstall = async () => {
    if (!bridge?.uninstallCli) return
    setBusy('uninstall')
    setError('')
    try {
      const next = await bridge.uninstallCli()
      setStatus(next)
      notify(t('config:cliSettings.uninstalledNotification'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const loading = supported && !status
  const stateLabel = !supported
    ? t('config:cliSettings.unavailable')
    : loading
      ? t('config:cliSettings.checking')
      : !status?.supported
        ? t('config:cliSettings.unavailable')
        : status.needsRepair
          ? t('config:cliSettings.needsRepair')
          : status.installed
            ? t('config:cliSettings.installed')
            : t('config:cliSettings.notInstalled')

  return (
    <Panel className="py-4" data-config-card="updates-cli">
      <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] items-start gap-3">
        <span className="grid size-9.5 shrink-0 place-items-center rounded-lg border border-key-border bg-key text-content shadow-key">
          <Terminal size={18} aria-hidden="true" />
        </span>
        <div className="min-w-0">
          <h2 className="text-[15px]! leading-6 font-medium text-content">
            {t('config:cliSettings.title')}
          </h2>
          <p className="mt-0.5 text-xs leading-[1.55] text-content-muted">
            {t('config:cliSettings.description')}
          </p>
        </div>
        <Badge
          className="self-center"
          tone={
            status?.installed && !status.needsRepair
              ? 'green'
              : status?.needsRepair
                ? 'blue'
                : 'gray'
          }
        >
          {stateLabel}
        </Badge>
      </div>

      {status && (
        <AppNotice className="mt-4 [&_small]:[overflow-wrap:anywhere] [&_small]:font-mono [&_small]:text-xs">
          {status.installed && !status.needsRepair ? (
            <CheckCircle2 size={16} />
          ) : (
            <Terminal size={16} />
          )}
          <span>
            <strong>
              {status.installed
                ? t('config:cliSettings.commandReady', { command: status.command })
                : t('config:cliSettings.commandNotInstalled', { command: status.command })}
            </strong>
            <small>{status.installPath}</small>
            {status.installed && (
              <small>
                {status.pathConfigured
                  ? t('config:cliSettings.pathConfigured')
                  : t('config:cliSettings.pathNeedsRepair')}
              </small>
            )}
          </span>
        </AppNotice>
      )}

      {error && <AppError>{error}</AppError>}

      {supported && (
        <div className="mt-3.5 flex justify-end gap-2 max-[650px]:flex-wrap">
          {status?.installed && (
            <Button
              type="button"
              variant="secondary"
              className="max-[650px]:h-11"
              disabled={Boolean(busy)}
              onClick={() => void uninstall()}
            >
              {busy === 'uninstall' ? (
                <OrbitSpinner className="text-current" />
              ) : (
                <Trash2 size={14} />
              )}
              {t('config:cliSettings.uninstall')}
            </Button>
          )}
          {(!status?.installed || status.needsRepair) && (
            <Button
              type="button"
              className="max-[650px]:h-11"
              disabled={Boolean(busy) || loading || status?.supported === false}
              onClick={() => void install()}
            >
              {busy === 'install' ? (
                <OrbitSpinner className="text-current" />
              ) : status?.needsRepair ? (
                <Wrench size={14} />
              ) : (
                <Download size={14} />
              )}
              {status?.needsRepair
                ? t('config:cliSettings.repair')
                : t('config:cliSettings.install')}
            </Button>
          )}
        </div>
      )}
      {supported && (
        <small className="mt-2 block text-xs text-content-muted">
          {t('config:cliSettings.restartTerminal')}
        </small>
      )}
    </Panel>
  )
}
