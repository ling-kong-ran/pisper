import { useEffect, useMemo, useState } from 'react'
import {
  CheckCircle2,
  Download,
  ExternalLink,
  Laptop,
  PackageCheck,
  RefreshCw,
  Rocket,
  TriangleAlert,
} from 'lucide-react'
import MarkdownMessage from '../../components/MarkdownMessage'
import { Badge, Panel, SectionTitle } from '../../components/ui'
import { useI18n } from '../../app/use-i18n'
import type { Notify } from '../../app/route-context'
import type { I18nValues, SupportedLanguage } from '../../app/i18n'
import type { AppUpdateController, AppUpdateInfo } from '../../types/update'

const BUILD_VERSION = import.meta.env.VITE_APP_VERSION || '0.0.0'

type Translate = (message: string, values?: I18nValues) => string

function platformLabel(info: AppUpdateInfo, t: Translate) {
  if (!info.desktop) return t('config:updateSettings.browserMode')
  const platform =
    ({ win32: 'Windows', darwin: 'macOS', linux: 'Linux' } as Record<string, string>)[
      info.platform
    ] || info.platform
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
  const [bundled, setBundled] = useState({ version: BUILD_VERSION, date: '', notes: '' })

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

  const check = async () => {
    await update?.check()
  }

  const openReleases = () => update?.openReleases()
  const openUpdateLog = () => update?.openUpdateLog?.()

  const download = async () => {
    try {
      await update?.download()
    } catch (error) {
      notify(error instanceof Error ? error.message : String(error), 'error')
    }
  }

  const install = () => update?.install()

  const notes =
    status.notes ||
    (desktop
      ? bundled.notes || t('config:updateSettings.noReleaseNotesAreAvailableForThisVersion')
      : status.state === 'current'
        ? t('config:updateSettings.theCurrentWebSourceIsSyncedWithBranch', {
            branch: status.branch || 'main',
          })
        : t('config:updateSettings.commitsThatHaveNotBeenSyncedWillAppearHereAfterTheCheck'))
  const available = status.state === 'available'
  const resumable = status.state === 'error' && status.canResume && status.canDownload
  const downloaded = status.state === 'downloaded'
  const checking = status.state === 'checking'
  const downloading = status.state === 'downloading'
  const currentIdentifier = desktop
    ? `v${info.version}`
    : `v${info.version}${status.currentCommit ? ` · ${status.currentCommit.slice(0, 7)}` : ''}`
  const latestIdentifier = desktop
    ? `v${status.availableVersion || bundled.version || info.version}`
    : status.availableCommit?.slice(0, 7) ||
      status.currentCommit?.slice(0, 7) ||
      bundled.version ||
      info.version
  const statusMeta = useMemo(
    () =>
      (
        ({
          idle: [t('config:updateSettings.notChecked'), 'gray'],
          checking: [t('config:updateSettings.checking'), 'blue'],
          current: [t('config:updateSettings.upToDate'), 'green'],
          available: [
            desktop
              ? t('config:updateSettings.updateAvailable')
              : t('config:updateSettings.sourceUpdatesAvailable'),
            'blue',
          ],
          downloading: [t('config:updateSettings.downloading'), 'blue'],
          downloaded: [t('config:updateSettings.readyToRestart'), 'green'],
          error: [
            resumable
              ? t('config:updateSettings.downloadPaused')
              : t('config:updateSettings.checkFailed'),
            'red',
          ],
        }) as Record<string, [string, 'gray' | 'blue' | 'green' | 'red']>
      )[status.state] || [t('config:updateSettings.notChecked'), 'gray'],
    [desktop, resumable, status.state, t],
  )

  return (
    <div className="mx-auto flex w-full max-w-[880px] flex-col gap-3">
      <Panel className="p-5">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="flex min-w-0 items-start gap-3">
            <span className="language-settings-icon">
              <PackageCheck size={19} />
            </span>
            <span className="min-w-0">
              <strong className="block text-[16px]">
                {t('config:updateSettings.vesperAppUpdates')}
              </strong>
            </span>
          </div>
          <Badge tone={statusMeta[1]}>{statusMeta[0]}</Badge>
        </div>
        <div className="mt-5 grid gap-2 sm:grid-cols-3">
          <div className="rounded-[var(--r-sm)] bg-[var(--surface-muted)] p-3">
            <small className="text-[12px] text-[var(--text-muted)]">
              {desktop
                ? t('config:updateSettings.currentVersion')
                : t('config:updateSettings.currentSource')}
            </small>
            <strong className="mt-1 block font-mono text-[14px]">{currentIdentifier}</strong>
          </div>
          <div className="rounded-[var(--r-sm)] bg-[var(--surface-muted)] p-3">
            <small className="text-[12px] text-[var(--text-muted)]">
              {t('config:updateSettings.platform')}
            </small>
            <strong className="mt-1 block text-[14px]">{platformLabel(info, t)}</strong>
          </div>
          <div className="rounded-[var(--r-sm)] bg-[var(--surface-muted)] p-3">
            <small className="text-[12px] text-[var(--text-muted)]">
              {t('config:updateSettings.updateChannel')}
            </small>
            <strong className="mt-1 block text-[14px]">
              {desktop ? 'Stable' : status.branch || 'main'}
            </strong>
          </div>
        </div>
        {status.message && (
          <div
            className={`mt-4 flex items-start gap-2 rounded-[var(--r-sm)] p-3 text-[13px] ${status.state === 'error' ? 'bg-[var(--danger-soft)] text-[var(--danger)]' : 'bg-[var(--accent-soft)] text-[var(--text)]'}`}
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
          <small className="mt-2 block text-[12px] text-[var(--text-muted)]">
            {t(
              'config:updateSettings.downloadedDataWasPreservedAndCanResumeFromTheInterruptionPoint',
            )}
          </small>
        )}
        {downloading && (
          <div className="mt-4">
            <div className="flex justify-between text-[12px] text-[var(--text-muted)]">
              <span>{t('config:updateSettings.downloadProgress')}</span>
              <span>
                {Math.round(status.percent || 0)}% · {formatBytes(status.transferred, language)} /{' '}
                {formatBytes(status.total, language)}
              </span>
            </div>
            <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-[var(--progress-track)]">
              <i
                className="block h-full bg-[var(--star)] transition-[width]"
                style={{ width: `${status.percent || 0}%` }}
              />
            </div>
          </div>
        )}
        <div className="mt-5 flex flex-wrap gap-2">
          <button
            className="button primary"
            disabled={checking || downloading}
            onClick={downloaded ? install : available || resumable ? download : check}
          >
            {checking || downloading ? (
              <RefreshCw className="spin" size={14} />
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
            {downloaded
              ? t('config:updateSettings.restartAndInstall')
              : resumable
                ? t('config:updateSettings.resumeDownload')
                : available
                  ? status.canDownload
                    ? t('config:updateSettings.downloadUpdate')
                    : desktop
                      ? t('config:updateSettings.viewRelease')
                      : t('config:updateSettings.viewSourceUpdates')
                  : checking
                    ? t('config:updateSettings.checking2')
                    : t('config:updateSettings.checkForUpdates')}
          </button>
          <button className="button secondary" onClick={openReleases}>
            <ExternalLink size={14} />
            {desktop ? 'GitHub Releases' : 'GitHub Compare'}
          </button>
          {desktop && (
            <button className="button secondary" onClick={openUpdateLog}>
              <ExternalLink size={14} />
              {t('config:updateSettings.viewUpdateDiagnosticLog')}
            </button>
          )}
        </div>
      </Panel>

      <Panel className="p-5">
        <div className="flex items-center justify-between gap-3">
          <SectionTitle
            title={
              desktop
                ? available || resumable || downloaded || downloading
                  ? t('config:updateSettings.newVersionReleaseNotes')
                  : t('config:updateSettings.currentReleaseNotes')
                : available
                  ? t('config:updateSettings.commitsToSync')
                  : t('config:updateSettings.currentSourceStatus')
            }
          />
          <span className="flex items-center gap-1.5 text-[12px] text-[var(--text-muted)]">
            <CheckCircle2 size={13} />
            {available || resumable || downloaded || downloading
              ? latestIdentifier
              : desktop
                ? `v${bundled.version || info.version}`
                : status.currentCommit?.slice(0, 7) || t('config:updateSettings.notChecked')}
          </span>
        </div>
        {(status.releaseDate || (desktop && bundled.date)) && (
          <small className="mt-2 block text-[12px] text-[var(--text-muted)]">
            {new Intl.DateTimeFormat(language, { dateStyle: 'long' }).format(
              new Date(status.releaseDate || bundled.date),
            )}
          </small>
        )}
        <div className="mt-4 rounded-[var(--r-sm)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)] p-4">
          <MarkdownMessage>{notes}</MarkdownMessage>
        </div>
      </Panel>
    </div>
  )
}
