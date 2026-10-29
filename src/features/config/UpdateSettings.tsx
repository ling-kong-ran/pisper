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
  PackageCheck,
  RefreshCw,
  Rocket,
  SquareTerminal,
  TriangleAlert,
} from 'lucide-react'
import { DEFAULT_BRANCH } from '@shared/app-update.mjs'
import MarkdownMessage from '@/components/MarkdownMessage'
import { CliSettings } from './CliSettings'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsSectionTitle as SectionTitle,
} from './settings-primitives'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { I18nValues, SupportedLanguage } from '@/app/i18n'
import type { AppUpdateController, AppUpdateInfo } from '@/types/update'

import { Button } from '@/components/ui/button'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'

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
  const statusMeta = useMemo(
    () =>
      (
        ({
          idle: [t('config:updateSettings.notChecked'), 'gray'],
          checking: [t('config:updateSettings.checking'), 'blue'],
          current: [t('config:updateSettings.upToDate'), 'green'],
          available: [
            nativeApp
              ? t('config:updateSettings.updateAvailable')
              : t('config:updateSettings.sourceUpdatesAvailable'),
            'blue',
          ],
          downloading: [t('config:updateSettings.downloading'), 'blue'],
          downloaded: [t('config:updateSettings.readyToRestart'), 'green'],
          installed: [t('config:updateSettings.upToDate'), 'green'],
          error: [
            resumable
              ? t('config:updateSettings.downloadPaused')
              : t('config:updateSettings.checkFailed'),
            'red',
          ],
        }) as Record<string, [string, 'gray' | 'blue' | 'green' | 'red']>
      )[overallState] || [t('config:updateSettings.notChecked'), 'gray'],
    [nativeApp, overallState, resumable, t],
  )

  return (
    <div className="mx-auto flex w-full flex-col gap-4">
      <Panel className="p-0 py-0" data-config-card="updates-main">
        <div className="flex flex-wrap items-center justify-between gap-3 border-b border-stroke-soft px-4 py-3.5">
          <div className="flex min-w-0 items-center gap-3">
            <span className="grid size-9.5 shrink-0 place-items-center rounded-lg border border-key-border bg-key text-content shadow-key">
              <PackageCheck size={18} aria-hidden="true" />
            </span>
            <h2 className="min-w-0 text-[15px]! leading-6 font-medium text-content">
              {t('config:updateSettings.pisperAppUpdates')}
            </h2>
          </div>
          <Badge tone={statusMeta[1]}>{statusMeta[0]}</Badge>
        </div>
        {/* 版本 / 平台 / 通道三格读数：细线分隔的仪表格，版本与通道用等宽字。 */}
        <dl className="grid divide-stroke-soft border-b border-stroke-soft max-sm:divide-y sm:grid-cols-3 sm:divide-x">
          <div className="px-4 py-3">
            <dt className="text-xs text-content-muted">
              {nativeApp
                ? t('config:updateSettings.currentVersion')
                : t('config:updateSettings.currentSource')}
            </dt>
            <dd className="mt-1 font-mono text-[12.5px] font-medium text-content">
              {currentIdentifier}
            </dd>
          </div>
          <div className="px-4 py-3">
            <dt className="text-xs text-content-muted">{t('config:updateSettings.platform')}</dt>
            <dd className="mt-1 text-sm font-medium text-content">{platformLabel(info, t)}</dd>
          </div>
          <div className="px-4 py-3">
            <dt className="text-xs text-content-muted">
              {t('config:updateSettings.updateChannel')}
            </dt>
            <dd className="mt-1 font-mono text-[12.5px] font-medium text-content">
              {nativeApp ? 'Stable' : status.branch || DEFAULT_BRANCH}
            </dd>
          </div>
        </dl>
        <div className="p-4">
          {status.message && (
            <div
              className={`flex items-start gap-2 rounded-sm p-2.5 text-sm ${status.state === 'error' ? 'bg-danger-soft text-danger' : 'bg-surface-subtle text-content-secondary'}`}
            >
              {status.state === 'error' ? (
                <TriangleAlert className="mt-0.5 shrink-0" size={15} />
              ) : (
                <Laptop className="mt-0.5 shrink-0" size={15} />
              )}
              <span>{status.message}</span>
            </div>
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
          <div className={cn('flex flex-wrap gap-2', (status.message || downloading) && 'mt-4')}>
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
        </div>
        {!mobile && (
          <div className="border-t border-stroke" data-config-card="updates-components">
            <div className="flex h-9 items-center bg-surface-subtle px-4">
              <SectionTitle
                title={t('config:updateSettings.appComponents')}
                className="text-xs font-medium text-content-muted"
              />
            </div>
            <div className="divide-y divide-stroke-soft border-t border-stroke-soft px-4">
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
                const tone =
                  component.state === 'error'
                    ? 'red'
                    : component.state === 'available'
                      ? 'blue'
                      : component.state === 'installed' || component.state === 'current'
                        ? 'green'
                        : 'gray'
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
                  <div
                    className="grid min-h-12 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 py-2.5 sm:grid-cols-[minmax(0,1fr)_auto_auto]"
                    key={component.component}
                  >
                    <div className="col-span-2 flex min-w-0 items-center gap-3 sm:col-span-1">
                      <Icon className="shrink-0 text-content-muted" size={16} aria-hidden="true" />
                      <div className="min-w-0">
                        <span className="block text-sm font-medium text-content">{label}</span>
                        {component.message && (
                          <small className="mt-0.5 block text-xs text-content-muted">
                            {component.message}
                          </small>
                        )}
                      </div>
                    </div>
                    <div className="flex items-center gap-2 font-mono text-xs text-content-secondary tabular-nums">
                      <span>v{component.currentVersion}</span>
                      {component.availableVersion &&
                        component.availableVersion !== component.currentVersion && (
                          <>
                            <ArrowRight size={12} />
                            <span>v{component.availableVersion}</span>
                          </>
                        )}
                      {component.size > 0 && component.state === 'available' && (
                        <span className="font-sans text-content-muted">
                          {formatBytes(component.size, language)}
                        </span>
                      )}
                    </div>
                    <Badge tone={tone}>{stateLabel}</Badge>
                  </div>
                )
              })}
            </div>
          </div>
        )}
      </Panel>

      {!mobile && <CliSettings notify={notify} />}

      <Panel className="p-4 py-4">
        <div className="flex items-center justify-between gap-3">
          <SectionTitle
            title={
              nativeApp
                ? available || resumable || downloaded || downloading
                  ? t('config:updateSettings.newVersionReleaseNotes')
                  : t('config:updateSettings.currentReleaseNotes')
                : available
                  ? t('config:updateSettings.commitsToSync')
                  : t('config:updateSettings.currentSourceStatus')
            }
          />
          <span className="flex items-center gap-1.5 font-mono text-xs text-content-muted">
            <CheckCircle2 size={13} aria-hidden="true" />
            {available || resumable || downloaded || downloading
              ? latestIdentifier
              : nativeApp
                ? `v${bundledCurrent?.version || info.version}`
                : status.currentCommit?.slice(0, 7) || t('config:updateSettings.notChecked')}
          </span>
        </div>
        {(status.releaseDate || (nativeApp && bundledCurrent?.date)) && (
          <small className="mt-1 block text-xs text-content-muted tabular-nums">
            {new Intl.DateTimeFormat(language, { dateStyle: 'long' }).format(
              new Date(status.releaseDate || bundledCurrent?.date || ''),
            )}
          </small>
        )}
        <div className="mt-3 rounded-sm border border-stroke-soft bg-surface-subtle p-4">
          <MarkdownMessage>{notes}</MarkdownMessage>
        </div>
      </Panel>
    </div>
  )
}
