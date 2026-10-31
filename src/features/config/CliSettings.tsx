// CLI 安装/修复设置：检测系统 pisper CLI 状态，提供安装/卸载/修复。
import { CheckCircle2, Download, Terminal, Trash2, Wrench } from 'lucide-react'
import { useEffect, useState } from 'react'
import { useI18n } from '@/app/use-i18n'
import { FeatureAlert, FeatureSection } from '@/components/app/FeaturePage'
import type { Notify } from '@/app/route-context'
import type { DesktopCliStatus } from '@/types/update'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'

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

  // 与更新页其他分区同一结构（分区标题 + 细线内容，左缘与页标题对齐）；状态读数中性。
  return (
    <FeatureSection
      data-config-card="updates-cli"
      title={t('config:cliSettings.title')}
      description={t('config:cliSettings.description')}
      actions={<span className="text-xs text-content-secondary">{stateLabel}</span>}
    >
      {status && (
        <div className="flex min-w-0 items-start gap-3 border-t border-stroke py-3">
          {status.installed && !status.needsRepair ? (
            <CheckCircle2
              className="mt-0.5 shrink-0 text-content-muted"
              size={16}
              aria-hidden="true"
            />
          ) : (
            <Terminal className="mt-0.5 shrink-0 text-content-muted" size={16} aria-hidden="true" />
          )}
          <div className="grid min-w-0 gap-0.5">
            <strong className="text-sm font-medium text-content">
              {status.installed
                ? t('config:cliSettings.commandReady', { command: status.command })
                : t('config:cliSettings.commandNotInstalled', { command: status.command })}
            </strong>
            <small className="font-mono text-xs [overflow-wrap:anywhere] text-content-muted">
              {status.installPath}
            </small>
            {status.installed && (
              <small className="text-xs text-content-muted">
                {status.pathConfigured
                  ? t('config:cliSettings.pathConfigured')
                  : t('config:cliSettings.pathNeedsRepair')}
              </small>
            )}
          </div>
        </div>
      )}

      {error && <FeatureAlert>{error}</FeatureAlert>}

      {supported && (
        <div className="mt-3 flex gap-2 max-[650px]:flex-wrap">
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
    </FeatureSection>
  )
}
