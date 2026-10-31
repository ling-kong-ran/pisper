// 更新设置：检查/下载/安装桌面端与组件（TUI/Runtime）更新，
// 展示版本、变更日志与安装进度。
import { useEffect, useMemo, useState } from 'react'
import {
  ArrowRight,
  CheckCircle2,
  Cpu,
  Download,
  ExternalLink,
  Laptop,
  RefreshCw,
  Rocket,
  SquareTerminal,
} from 'lucide-react'
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
  const [componentBusy, setComponentBusy] = useState<'check' | 'install' | ''>('')

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

  // 检查更新：调控制器检查并防重入（busy 标记）。
  const check = async () => {
    if (!update) return
    setComponentBusy('check')
    try {
      await update.check()
    } finally {
      setComponentBusy('')
    }
  }

  const openReleases = () => update?.openReleases()
  const openUpdateLog = () => update?.openUpdateLog?.()

  // 下载/安装更新：桌面端安装全部组件，Web 端仅下载；失败提示。
  const download = async () => {
    setComponentBusy(desktop ? 'install' : '')
    try {
      if (desktop) await update?.installComponents()
      else await update?.download()
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    } finally {
      setComponentBusy('')
    }
  }

  const install = () => update?.install()

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
  const available = status.state === 'available'
  const resumable = status.state === 'error' && status.canResume && status.canDownload
  const downloaded = status.state === 'downloaded'
  const checking = status.state === 'checking' || componentBusy === 'check'
  const downloading = status.state === 'downloading' || componentBusy === 'install'
  const overallState = checking ? 'checking' : downloading ? 'downloading' : status.state
  const currentIdentifier = nativeApp
    ? `v${info.version}`
    : `v${info.version}${status.currentCommit ? ` · ${status.currentCommit.slice(0, 7)}` : ''}`
  const latestIdentifier = nativeApp
    ? `v${status.availableVersion || bundledCurrent?.version || info.version}`
    : status.availableCommit?.slice(0, 7) ||
      status.currentCommit?.slice(0, 7) ||
      bundled.version ||
      info.version
  const componentItems = useMemo(() => {
    const byName = new Map(update.components.map((component) => [component.component, component]))
    return (['desktop', 'tui', 'runtime'] as const).map(
      (component) =>
        byName.get(component) || {
          component,
          state: 'idle',
          currentVersion: component === 'desktop' ? info.version : '--',
          availableVersion: '',
          message: '',
          releaseUrl: '',
          notes: '',
          size: 0,
          transferred: 0,
          canInstall: false,
          restartRequired: false,
        },
    )
  }, [info.version, update.components])
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
        error: resumable
          ? t('config:updateSettings.downloadPaused')
          : t('config:updateSettings.checkFailed'),
      } as Record<string, string>
    )[overallState] || t('config:updateSettings.notChecked')

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
            <FeatureAlert>{status.message}</FeatureAlert>
          ) : (
            <p className="mt-4 flex items-start gap-2 text-sm text-content-secondary">
              <Laptop className="mt-0.5 shrink-0 text-content-muted" size={15} aria-hidden="true" />
              <span className="min-w-0">{status.message}</span>
            </p>
          ))}
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
                {Math.round(status.percent || 0)}% · {formatBytes(status.transferred, language)} /{' '}
                {formatBytes(status.total, language)}
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
          <Button
            className="max-[650px]:h-11"
            disabled={checking || downloading || Boolean(componentBusy)}
            onClick={downloaded ? install : available || resumable ? download : check}
          >
            {checking || downloading ? (
              <OrbitSpinner className="text-current" />
            ) : downloaded ? (
              <Rocket size={14} />
            ) : available || resumable ? (
              status.canDownload ? (
                <Download size={14} />
              ) : (
                <ExternalLink size={14} />
              )
            ) : (
              <RefreshCw size={14} />
            )}
            {checking || downloading
              ? downloading && desktop
                ? t('config:updateSettings.installingUpdates')
                : t('config:updateSettings.checking2')
              : downloaded
                ? t('config:updateSettings.restartAndInstall')
                : resumable
                  ? t('config:updateSettings.resumeDownload')
                  : available
                    ? status.canDownload
                      ? desktop
                        ? t('config:updateSettings.downloadAndInstallUpdates')
                        : t('config:updateSettings.downloadUpdate')
                      : nativeApp
                        ? t('config:updateSettings.viewRelease')
                        : t('config:updateSettings.viewSourceUpdates')
                    : t('config:updateSettings.checkForUpdates')}
          </Button>
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
      </FeatureSection>

      {!mobile && (
        <FeatureSection
          data-config-card="updates-components"
          title={t('config:updateSettings.appComponents')}
        >
          <FeatureList>
            {componentItems.map((component) => {
              const Icon =
                component.component === 'desktop'
                  ? Laptop
                  : component.component === 'tui'
                    ? SquareTerminal
                    : Cpu
              const label =
                component.component === 'desktop'
                  ? t('config:updateSettings.desktopFrontend')
                  : component.component === 'tui'
                    ? t('config:updateSettings.tuiClient')
                    : t('config:updateSettings.runtime')
              const stateLabel =
                component.state === 'checking'
                  ? t('config:updateSettings.checking')
                  : component.state === 'downloading'
                    ? t('config:updateSettings.downloading')
                    : component.state === 'available'
                      ? t('config:updateSettings.updateAvailable')
                      : component.state === 'installed'
                        ? t('config:updateSettings.installed')
                        : component.state === 'error'
                          ? t('config:updateSettings.checkFailed')
                          : component.state === 'current'
                            ? t('config:updateSettings.upToDate')
                            : t('config:updateSettings.notChecked')
              return (
                <FeatureRow
                  key={component.component}
                  icon={<Icon />}
                  name={label}
                  description={component.message || undefined}
                  meta={
                    <span className="flex items-center gap-1.5 font-mono tabular-nums">
                      <span>v{component.currentVersion}</span>
                      {component.availableVersion &&
                        component.availableVersion !== component.currentVersion && (
                          <>
                            <ArrowRight size={12} aria-hidden="true" />
                            <span>v{component.availableVersion}</span>
                          </>
                        )}
                      {component.size > 0 && component.state === 'available' && (
                        <span className="font-sans">{formatBytes(component.size, language)}</span>
                      )}
                    </span>
                  }
                  status={stateLabel}
                />
              )
            })}
          </FeatureList>
        </FeatureSection>
      )}

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
