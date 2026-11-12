// 设置 › 更新里的“桌面安装包”分区：组件更新换不了桌面程序本身，壳在后台下载并验签新版安装包，
// 这里显示下载状态，并提供“安装”（用系统打开安装包，由用户完成）、“在文件夹中显示”和自动下载开关。
import { useState } from 'react'
import { Download, ExternalLink, FolderOpen, PackageCheck, RefreshCw, Rocket } from 'lucide-react'
import { FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { SupportedLanguage } from '@/app/i18n'
import type { ShellInstallerController } from '@/types/update'

function formatBytes(value: number, language: SupportedLanguage) {
  if (!value) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  const level = Math.min(units.length - 1, Math.floor(Math.log(value) / Math.log(1024)))
  return `${(value / 1024 ** level).toLocaleString(language, { maximumFractionDigits: 1 })} ${units[level]}`
}

export function ShellInstallerSettings({
  installer,
  notify,
  openReleases,
}: {
  installer: ShellInstallerController
  notify: Notify
  openReleases: () => unknown
}) {
  const { t, language } = useI18n()
  const [busy, setBusy] = useState<'' | 'open' | 'reveal' | 'toggle' | 'download' | 'check'>('')
  const status = installer.status
  const version = status.availableVersion ? `v${status.availableVersion}` : ''
  // 服务器没给总大小时只显示已下载的字节数。
  const percent = status.size ? Math.min(100, (status.transferred / status.size) * 100) : null

  const run = async (kind: Exclude<typeof busy, ''>, action: () => Promise<unknown>) => {
    setBusy(kind)
    try {
      await action()
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      notify(t('config:updateSettings.installerActionFailed', { message }), 'error')
    } finally {
      setBusy('')
    }
  }

  // 状态读数一律中性（规格 6.1）：失败是已经发生的结果，当前版本照常可用。
  const view = (() => {
    switch (status.state) {
      case 'downloaded':
        return {
          icon: <PackageCheck />,
          name: t('config:updateSettings.installerReady', { version }),
          description: t('config:updateSettings.installerReadyHint'),
        }
      case 'downloading':
        return {
          icon: <OrbitSpinner className="text-current" />,
          name: t('config:updateSettings.installerDownloading', { version }),
          description:
            percent === null
              ? formatBytes(status.transferred, language)
              : `${Math.round(percent)}% · ${formatBytes(status.transferred, language)} / ${formatBytes(status.size, language)}`,
        }
      case 'available':
        return status.canDownload
          ? {
              icon: <Download />,
              name: t('config:updateSettings.installerAvailable', { version }),
              description: status.autoDownload
                ? undefined
                : t('config:updateSettings.installerAvailableHint'),
            }
          : {
              icon: <ExternalLink />,
              name: t('config:updateSettings.installerManual', { version }),
              description: t('config:updateSettings.installerManualHint'),
            }
      case 'checking':
        return {
          icon: <OrbitSpinner className="text-current" />,
          name: t('config:updateSettings.installerChecking'),
          description: undefined,
        }
      case 'current':
        return {
          icon: <PackageCheck />,
          name: t('config:updateSettings.installerCurrent'),
          description: undefined,
        }
      case 'error':
        return {
          icon: <RefreshCw />,
          name: t('config:updateSettings.installerFailed'),
          description: status.retryAt
            ? t('config:updateSettings.githubRateLimitedUntil', {
                time: new Intl.DateTimeFormat(language, { timeStyle: 'short' }).format(
                  new Date(status.retryAt * 1000),
                ),
              })
            : t('config:updateSettings.installerFailedHint'),
        }
      default:
        return {
          icon: <PackageCheck />,
          name: t('config:updateSettings.installerIdle'),
          description: undefined,
        }
    }
  })()

  const actions =
    status.state === 'downloaded' ? (
      <>
        <Button
          className="max-[650px]:h-11"
          disabled={Boolean(busy)}
          onClick={() => void run('open', installer.open)}
        >
          {busy === 'open' ? <OrbitSpinner className="text-current" /> : <Rocket size={14} />}
          {t('config:updateSettings.installerInstall')}
        </Button>
        <Button
          variant="secondary"
          className="max-[650px]:h-11"
          disabled={Boolean(busy)}
          onClick={() => void run('reveal', installer.reveal)}
        >
          <FolderOpen size={14} />
          {t('config:updateSettings.installerReveal')}
        </Button>
      </>
    ) : status.state === 'available' && status.canDownload && !status.autoDownload ? (
      <Button
        className="max-[650px]:h-11"
        disabled={Boolean(busy)}
        onClick={() => void run('download', installer.download)}
      >
        <Download size={14} />
        {t('config:updateSettings.installerDownload')}
      </Button>
    ) : status.state === 'available' && !status.canDownload ? (
      <Button variant="secondary" className="max-[650px]:h-11" onClick={() => void openReleases()}>
        <ExternalLink size={14} />
        GitHub Releases
      </Button>
    ) : status.state === 'error' ? (
      <Button
        variant="secondary"
        className="max-[650px]:h-11"
        disabled={Boolean(busy)}
        onClick={() => void run('check', status.canDownload ? installer.download : installer.check)}
      >
        <RefreshCw size={14} />
        {t('config:updateSettings.installerRetry')}
      </Button>
    ) : undefined

  return (
    <FeatureSection
      data-config-card="updates-installer"
      title={t('config:updateSettings.installerTitle')}
      description={t('config:updateSettings.installerDescription')}
    >
      <FeatureList>
        <FeatureRow
          data-shell-installer-state={status.state}
          icon={view.icon}
          name={view.name}
          nameMeta={
            status.currentVersion && version && status.state !== 'current'
              ? `v${status.currentVersion} → ${version}`
              : status.currentVersion
                ? `v${status.currentVersion}`
                : undefined
          }
          description={view.description}
          actions={actions}
          aria-live="polite"
        />
        <FeatureRow
          icon={<Download />}
          name={t('config:updateSettings.installerAutoDownload')}
          description={t('config:updateSettings.installerAutoDownloadHint')}
          actions={
            <Switch
              checked={status.autoDownload}
              disabled={busy === 'toggle'}
              onCheckedChange={(enabled) =>
                void run('toggle', () => installer.setAutoDownload(enabled))
              }
              aria-label={t('config:updateSettings.installerAutoDownload')}
            />
          }
        />
      </FeatureList>
    </FeatureSection>
  )
}
