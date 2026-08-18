// CLI 安装/修复设置：检测系统 pisper CLI 状态，提供安装/卸载/修复。
import { CheckCircle2, Download, RefreshCw, Terminal, Trash2, Wrench } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { SettingsBadge as Badge, SettingsCard as Panel } from './settings-primitives'
import type { Notify } from '@/app/route-context'
import type { DesktopCliStatus } from '@/types/update'

import { Button } from '@/components/ui/button'

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
    <Panel className="[padding:18px] cli-settings-card">
      <div className="language-settings-heading flex items-start gap-[11px] [&_h2]:text-[16px] [&_p]:mt-[4px] [&_p]:text-[var(--text-muted)] [&_p]:text-[13px] [&_p]:leading-[1.55] cli-settings-heading !grid grid-cols-[auto_minmax(0,1fr)_auto]">
        <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
          <Terminal size={19} />
        </span>
        <div>
          <h2>{t('config:cliSettings.title')}</h2>
          <p>{t('config:cliSettings.description')}</p>
        </div>
        <Badge
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
        <AppNotice className="[margin-top:15px] cli-settings-status [&_small]:[overflow-wrap:anywhere]">
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
              variant="outline"
              size="lg"
              className="bg-surface-subtle"
              disabled={Boolean(busy)}
              onClick={() => void uninstall()}
            >
              {busy === 'uninstall' ? (
                <RefreshCw className="animate-spin" size={14} />
              ) : (
                <Trash2 size={14} />
              )}
              {t('config:cliSettings.uninstall')}
            </Button>
          )}
          {(!status?.installed || status.needsRepair) && (
            <Button
              type="button"
              size="lg"
              disabled={Boolean(busy) || loading || status?.supported === false}
              onClick={() => void install()}
            >
              {busy === 'install' ? (
                <RefreshCw className="animate-spin" size={14} />
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
        <small className="block [margin:9px_1px_0] text-[var(--text-muted)] text-[12px]">
          {t('config:cliSettings.restartTerminal')}
        </small>
      )}
    </Panel>
  )
}
