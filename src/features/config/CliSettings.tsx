import { CheckCircle2, Download, RefreshCw, Terminal, Trash2, Wrench } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { Badge, Panel } from '@/components/ui'
import type { Notify } from '@/app/route-context'
import type { DesktopCliStatus } from '@/types/update'

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

  if (!bridge?.getCliStatus || !bridge.installCli || !bridge.uninstallCli) return null

  const install = async () => {
    setBusy('install')
    setError('')
    try {
      const next = await bridge.installCli!()
      setStatus(next)
      notify(t('config:cliSettings.installedNotification'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const uninstall = async () => {
    setBusy('uninstall')
    setError('')
    try {
      const next = await bridge.uninstallCli!()
      setStatus(next)
      notify(t('config:cliSettings.uninstalledNotification'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setBusy('')
    }
  }

  const loading = !status
  const stateLabel = loading
    ? t('config:cliSettings.checking')
    : !status.supported
      ? t('config:cliSettings.unavailable')
      : status.needsRepair
        ? t('config:cliSettings.needsRepair')
        : status.installed
          ? t('config:cliSettings.installed')
          : t('config:cliSettings.notInstalled')

  return (
    <Panel className="language-settings-card cli-settings-card">
      <div className="language-settings-heading cli-settings-heading">
        <span className="language-settings-icon">
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
        <div className="permission-note language-settings-note cli-settings-status">
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
        </div>
      )}

      {error && <div className="config-error">{error}</div>}

      <div className="button-row cli-settings-actions">
        {status?.installed && (
          <button
            type="button"
            className="button secondary"
            disabled={Boolean(busy)}
            onClick={() => void uninstall()}
          >
            {busy === 'uninstall' ? <RefreshCw className="spin" size={14} /> : <Trash2 size={14} />}
            {t('config:cliSettings.uninstall')}
          </button>
        )}
        {(!status?.installed || status.needsRepair) && (
          <button
            type="button"
            className="button primary"
            disabled={Boolean(busy) || loading || status?.supported === false}
            onClick={() => void install()}
          >
            {busy === 'install' ? (
              <RefreshCw className="spin" size={14} />
            ) : status?.needsRepair ? (
              <Wrench size={14} />
            ) : (
              <Download size={14} />
            )}
            {status?.needsRepair ? t('config:cliSettings.repair') : t('config:cliSettings.install')}
          </button>
        )}
      </div>
      <small className="language-settings-storage">{t('config:cliSettings.restartTerminal')}</small>
    </Panel>
  )
}
