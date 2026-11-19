// 更新设置：检查、下载并安装 Pisper 的新版本（桌面端整体更新桌面程序，手机去 App 更新页，
// 浏览器比对源码分支），展示版本、变更日志与下载进度。
import { useEffect, useState } from 'react'
import { CheckCircle2, Download, ExternalLink, Laptop, RefreshCw, Rocket } from 'lucide-react'
import { DEFAULT_BRANCH } from '@shared/app-update.mjs'
import MarkdownMessage from '@/components/MarkdownMessage'
import { CliSettings } from './CliSettings'
import { FeatureAlert, FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { I18nValues, SupportedLanguage } from '@/app/i18n'
import type { AppUpdateController, AppUpdateInfo } from '@/types/update'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'

const BUILD_VERSION = import.meta.env.VITE_APP_VERSION || '0.0.0'

type Translate = (message: string, values?: I18nValues) => string

function platformLabel(info: AppUpdateInfo, t: Translate) {
  if (!info.desktop && !info.mobile) return t('config:updateSettings.browserMode')
  const platform =
    (
      {
        win32: 'Windows',
        darwin: 'macOS',
        linux: 'Linux',
        android: 'Android',
        ios: 'iOS',
      } as Record<string, string>
    )[info.platform] || info.platform
  return `${platform} · ${info.arch}`
}

function formatBytes(value: unknown, language: SupportedLanguage) {
  const bytes = Number(value) || 0
  if (!bytes) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  const level = Math.min(units.length - 1, Math.floor(Math.log(bytes) / Math.log(1024)))
  return `${(bytes / 1024 ** level).toLocaleString(language, { maximumFractionDigits: 1 })} ${units[level]}`
}

export function UpdateSettings({
  notify,
  update,
}: {
  notify: Notify
  update: AppUpdateController
}) {
  const { t, language } = useI18n()
  const info = update?.info || {
    desktop: false,
    packaged: false,
    version: BUILD_VERSION,
    platform: 'browser',
    arch: '',
  }
  const status = update?.status || { state: 'idle' }
  const desktop = Boolean(info.desktop)
  const mobile = Boolean(info.mobile)
  const nativeApp = desktop || mobile
  const [bundled, setBundled] = useState({ version: BUILD_VERSION, date: '', notes: '' })
  const [busy, setBusy] = useState<'' | 'check' | 'download' | 'install' | 'toggle'>('')

  useEffect(() => {
    let active = true
    fetch('/release-notes.json', { cache: 'no-store' })
      .then((response) => (response.ok ? response.json() : null))
      .then((value) => {
        if (active && value) setBundled(value)
      })
      .catch(() => {})
    return () => {
      active = false
    }
  }, [])

  // 用户操作同一时刻只跑一个；失败提示原因，状态以控制器（桌面端是壳）为准。
  const run = async (kind: Exclude<typeof busy, ''>, action: () => unknown) => {
    setBusy(kind)
    try {
      await action()
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    } finally {
      setBusy('')
    }
  }
  const check = () => run('check', () => update.check())
  // 桌面端由壳在后台下载更新包；手机和浏览器去下载页。
  const download = () => run('download', () => update.download())
  // 桌面端：重启并更新（v0.6.40 的壳是打开安装包）。
  const install = () => run('install', () => update.install())
  const openReleases = () => update?.openReleases()
  const openUpdateLog = () => update?.openUpdateLog?.()
  const shell = desktop ? update.shellInstaller : null

  const bundledCurrent = bundled.version === info.version ? bundled : null
  const notes =
    status.notes ||
    (nativeApp
      ? bundledCurrent?.notes || t('config:updateSettings.noReleaseNotesAreAvailableForThisVersion')
      : status.state === 'current'
        ? t('config:updateSettings.theCurrentWebSourceIsSyncedWithBranch', {
            branch: status.branch || DEFAULT_BRANCH,
          })
        : t('config:updateSettings.commitsThatHaveNotBeenSyncedWillAppearHereAfterTheCheck'))
  const rateLimitMessage = !status.rateLimited
    ? ''
    : status.retryAt
      ? t('config:updateSettings.githubRateLimitedUntil', {
          time: new Intl.DateTimeFormat(language, { timeStyle: 'short' }).format(
            new Date(status.retryAt * 1000),
          ),
        })
      : t('config:updateSettings.githubRateLimited')
  const unsupported = status.state === 'unsupported'
  const available = status.state === 'available'
  const resumable = status.state === 'error' && status.canResume && status.canDownload
  // 桌面端下载失败后可以直接重下，不用先再检查一次。
  const retryDownload = desktop && status.state === 'error' && status.canDownload
  const downloaded = status.state === 'downloaded'
  const checking = status.state === 'checking' || busy === 'check'
  const downloading = status.state === 'downloading'
  const installing = busy === 'install'
  const overallState = checking ? 'checking' : status.state
  const version = status.availableVersion ? `v${status.availableVersion}` : ''
  const currentIdentifier = nativeApp
    ? `v${info.version}`
    : `v${info.version}${status.currentCommit ? ` · ${status.currentCommit.slice(0, 7)}` : ''}`
  const latestIdentifier = nativeApp
    ? `v${status.availableVersion || bundledCurrent?.version || info.version}`
    : status.availableCommit?.slice(0, 7) ||
      status.currentCommit?.slice(0, 7) ||
      bundled.version ||
      info.version
  // 状态读数一律中性（规格 6.1）：检查失败、下载暂停是已经发生的结果，不用红色；没有“需要你”的状态，所以也不用琥珀。
  const statusLabel =
    (
      {
        idle: t('config:updateSettings.notChecked'),
        checking: t('config:updateSettings.checking'),
        current: t('config:updateSettings.upToDate'),
        available: nativeApp
          ? t('config:updateSettings.updateAvailable')
          : t('config:updateSettings.sourceUpdatesAvailable'),
        downloading: t('config:updateSettings.downloading'),
        downloaded: t('config:updateSettings.readyToRestart'),
        installed: t('config:updateSettings.upToDate'),
        unsupported: t('config:updateSettings.manualUpdateOnly'),
        error: resumable
          ? t('config:updateSettings.downloadPaused')
          : t('config:updateSettings.checkFailed'),
      } as Record<string, string>
    )[overallState] || t('config:updateSettings.notChecked')
  // 下好之后说清楚点了会发生什么：重启并更新会关掉 Pisper，v0.6.40 的壳则是打开安装包。
  const hint = unsupported
    ? t('config:updateSettings.desktopNeedsReinstall')
    : desktop && downloaded
      ? status.installKind === 'installer'
        ? t('config:updateSettings.installerReadyHint')
        : t('config:updateSettings.restartToUpdateHint')
      : ''

  // 方向 A 的设置分区（效果图 desktop-settings-light.png）：分区标题 + 细线行，左缘与页标题对齐，
  // 不再有带键帽图标的标题条、灰底分区头与 px-4 的内框。
  return (
    <div className="flex w-full min-w-0 flex-col">
      <FeatureSection
        data-config-card="updates-main"
        className="mt-6"
        title={t('config:updateSettings.pisperAppUpdates')}
        actions={
          <span
            data-update-status={overallState}
            className="text-xs text-content-secondary tabular-nums"
          >
            {statusLabel}
          </span>
        }
      >
        {/* 版本 / 平台 / 通道三格读数：细线分隔，版本与通道用等宽字。 */}
        <dl className="grid border-y border-stroke max-sm:divide-y max-sm:divide-stroke-soft sm:grid-cols-3">
          <div className="py-3 sm:pr-4">
            <dt className="text-xs text-content-muted">
              {nativeApp
                ? t('config:updateSettings.currentVersion')
                : t('config:updateSettings.currentSource')}
            </dt>
            <dd className="mt-1 font-mono text-[12.5px] font-medium text-content">
              {currentIdentifier}
            </dd>
          </div>
          <div className="py-3 sm:border-l sm:border-stroke-soft sm:px-4">
            <dt className="text-xs text-content-muted">{t('config:updateSettings.platform')}</dt>
            <dd className="mt-1 text-sm font-medium text-content">{platformLabel(info, t)}</dd>
          </div>
          <div className="py-3 sm:border-l sm:border-stroke-soft sm:pl-4">
            <dt className="text-xs text-content-muted">
              {t('config:updateSettings.updateChannel')}
            </dt>
            <dd className="mt-1 font-mono text-[12.5px] font-medium text-content">
              {nativeApp ? 'Stable' : status.branch || DEFAULT_BRANCH}
            </dd>
          </div>
        </dl>
        {status.message &&
          (status.state === 'error' ? (
            <FeatureAlert>{rateLimitMessage || status.message}</FeatureAlert>
          ) : (
            <p className="mt-4 flex items-start gap-2 text-sm text-content-secondary">
              <Laptop className="mt-0.5 shrink-0 text-content-muted" size={15} aria-hidden="true" />
              <span className="min-w-0">{status.message}</span>
            </p>
          ))}
        {hint && (
          <p className="mt-4 flex items-start gap-2 text-sm text-content-secondary">
            <Laptop className="mt-0.5 shrink-0 text-content-muted" size={15} aria-hidden="true" />
            <span className="min-w-0">{hint}</span>
          </p>
        )}
        {resumable && (
          <small className="mt-2 block text-xs text-content-muted">
            {t(
              'config:updateSettings.downloadedDataWasPreservedAndCanResumeFromTheInterruptionPoint',
            )}
          </small>
        )}
        {downloading && (
          <div className="mt-4">
            <div className="flex justify-between text-xs text-content-muted">
              <span>{t('config:updateSettings.downloadProgress')}</span>
              <span className="tabular-nums">
                {status.total
                  ? `${Math.round(status.percent || 0)}% · ${formatBytes(status.transferred, language)} / ${formatBytes(status.total, language)}`
                  : formatBytes(status.transferred, language)}
              </span>
            </div>
            <div className="mt-2 h-1 overflow-hidden rounded-full bg-surface-active">
              <i
                className="block h-full bg-star transition-[width]"
                style={{ width: `${status.percent || 0}%` }}
              />
            </div>
          </div>
        )}
        <div className="mt-4 flex flex-wrap gap-2">
          {!unsupported && (
            <Button
              className="max-[650px]:h-11"
              disabled={checking || downloading || Boolean(busy)}
              onClick={
                downloaded ? install : available || resumable || retryDownload ? download : check
              }
            >
              {checking || downloading || installing ? (
                <OrbitSpinner className="text-current" />
              ) : downloaded ? (
                <Rocket size={14} />
              ) : available || resumable || retryDownload ? (
                status.canDownload ? (
                  <Download size={14} />
                ) : (
                  <ExternalLink size={14} />
                )
              ) : (
                <RefreshCw size={14} />
              )}
              {checking
                ? t('config:updateSettings.checking2')
                : downloading
                  ? t('config:updateSettings.downloadingUpdate')
                  : downloaded
                    ? desktop
                      ? status.installKind === 'installer'
                        ? t('config:updateSettings.installVersion', { version })
                        : t('config:updateSettings.restartAndUpdate', { version })
                      : t('config:updateSettings.restartAndInstall')
                    : resumable
                      ? t('config:updateSettings.resumeDownload')
                      : retryDownload
                        ? t('config:updateSettings.retryDownload')
                        : available
                          ? status.canDownload
                            ? t('config:updateSettings.downloadUpdate')
                            : nativeApp
                              ? t('config:updateSettings.viewRelease')
                              : t('config:updateSettings.viewSourceUpdates')
                          : t('config:updateSettings.checkForUpdates')}
            </Button>
          )}
          <Button variant="secondary" className="max-[650px]:h-11" onClick={openReleases}>
            <ExternalLink size={14} />
            {nativeApp ? 'GitHub Releases' : 'GitHub Compare'}
          </Button>
          {desktop && (
            <Button variant="secondary" className="max-[650px]:h-11" onClick={openUpdateLog}>
              <ExternalLink size={14} />
              {t('config:updateSettings.viewUpdateDiagnosticLog')}
            </Button>
          )}
        </div>
        {shell && (
          <FeatureList className="mt-4">
            <FeatureRow
              icon={<Download />}
              name={t('config:updateSettings.autoDownload')}
              description={t('config:updateSettings.autoDownloadHint')}
              actions={
                <Switch
                  checked={shell.status.autoDownload}
                  disabled={busy === 'toggle'}
                  onCheckedChange={(enabled) =>
                    void run('toggle', () => shell.setAutoDownload(enabled))
                  }
                  aria-label={t('config:updateSettings.autoDownload')}
                />
              }
            />
          </FeatureList>
        )}
      </FeatureSection>

      {!mobile && <CliSettings notify={notify} />}

      <FeatureSection
        title={
          nativeApp
            ? available || resumable || downloaded || downloading
              ? t('config:updateSettings.newVersionReleaseNotes')
              : t('config:updateSettings.currentReleaseNotes')
            : available
              ? t('config:updateSettings.commitsToSync')
              : t('config:updateSettings.currentSourceStatus')
        }
        meta={
          (status.releaseDate || (nativeApp && bundledCurrent?.date)) &&
          new Intl.DateTimeFormat(language, { dateStyle: 'long' }).format(
            new Date(status.releaseDate || bundledCurrent?.date || ''),
          )
        }
        actions={
          <span className="flex items-center gap-1.5 font-mono text-xs text-content-muted">
            <CheckCircle2 size={13} aria-hidden="true" />
            {available || resumable || downloaded || downloading
              ? latestIdentifier
              : nativeApp
                ? `v${bundledCurrent?.version || info.version}`
                : status.currentCommit?.slice(0, 7) || t('config:updateSettings.notChecked')}
          </span>
        }
      >
        <div className="border-t border-stroke pt-3">
          <MarkdownMessage>{notes}</MarkdownMessage>
        </div>
      </FeatureSection>
    </div>
  )
}
