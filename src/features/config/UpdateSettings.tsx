// 更新设置：检查/下载/安装桌面端与组件（TUI/Runtime）更新，
// 展示版本、变更日志与安装进度。
import { useEffect, useMemo, useState } from 'react'
import {
  ArrowRight,
  CheckCircle2,
  Cpu,
  Download,
  ExternalLink,
  Handshake,
  Laptop,
  PackageCheck,
  RefreshCw,
  Rocket,
  SquareTerminal,
  TriangleAlert,
  X,
} from 'lucide-react'
import MarkdownMessage from '@/components/MarkdownMessage'
import { CliSettings } from './CliSettings'
import {
  SettingsBadge as Badge,
  SettingsCard as Panel,
  SettingsSectionTitle as SectionTitle,
} from './settings-primitives'
import { STORAGE_KEYS } from '@/app/storage'
import { useI18n } from '@/app/use-i18n'
import type { Notify } from '@/app/route-context'
import type { I18nValues, SupportedLanguage } from '@/app/i18n'
import { apiJson } from '@/lib/api'
import type { AppUpdateController, AppUpdateInfo } from '@/types/update'

import { Button } from '@/components/ui/button'

const BUILD_VERSION = import.meta.env.VITE_APP_VERSION || '0.0.0'
const SPONSOR_REFRESH_MS = 15 * 60_000
const SPONSOR_DISMISSAL_MS = 30 * 24 * 60 * 60_000

type Translate = (message: string, values?: I18nValues) => string

type SponsorCampaign = {
  id: string
  name: string
  description: string
  href: string
}

type SponsorResponse = {
  campaigns: SponsorCampaign[]
}

function storedSponsorDismissals() {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEYS.sponsorDismissals) || '{}')
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    return Object.fromEntries(
      Object.entries(value).filter((entry): entry is [string, number] => {
        return typeof entry[1] === 'number' && entry[1] > Date.now()
      }),
    )
  } catch {
    return {}
  }
}

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
  const [sponsors, setSponsors] = useState<SponsorCampaign[]>([])
  const [componentBusy, setComponentBusy] = useState<'check' | 'install' | ''>('')
  const [sponsorDismissals, setSponsorDismissals] =
    useState<Record<string, number>>(storedSponsorDismissals)

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

  useEffect(() => {
    let active = true
    const loadSponsors = (force = false) => {
      const refresh = force ? '&refresh=1' : ''
      return apiJson<SponsorResponse>(
        `/api/sponsors/settings-updates?locale=${encodeURIComponent(language)}${refresh}`,
      )
        .then((value) => {
          if (active) setSponsors(Array.isArray(value.campaigns) ? value.campaigns : [])
        })
        .catch(() => {
          if (active && !force) setSponsors([])
        })
    }
    void loadSponsors()
    const timer = window.setInterval(() => void loadSponsors(true), SPONSOR_REFRESH_MS)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [language])

  const dismissSponsor = (id: string) => {
    const next = { ...sponsorDismissals, [id]: Date.now() + SPONSOR_DISMISSAL_MS }
    setSponsorDismissals(next)
    try {
      localStorage.setItem(STORAGE_KEYS.sponsorDismissals, JSON.stringify(next))
    } catch {
      // The dismissal remains effective for this session when storage is unavailable.
    }
  }

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
    (desktop
      ? bundledCurrent?.notes || t('config:updateSettings.noReleaseNotesAreAvailableForThisVersion')
      : status.state === 'current'
        ? t('config:updateSettings.theCurrentWebSourceIsSyncedWithBranch', {
            branch: status.branch || 'main',
          })
        : t('config:updateSettings.commitsThatHaveNotBeenSyncedWillAppearHereAfterTheCheck'))
  const available = status.state === 'available'
  const resumable = status.state === 'error' && status.canResume && status.canDownload
  const downloaded = status.state === 'downloaded'
  const checking = status.state === 'checking' || componentBusy === 'check'
  const downloading = status.state === 'downloading' || componentBusy === 'install'
  const overallState = checking ? 'checking' : downloading ? 'downloading' : status.state
  const currentIdentifier = desktop
    ? `v${info.version}`
    : `v${info.version}${status.currentCommit ? ` · ${status.currentCommit.slice(0, 7)}` : ''}`
  const latestIdentifier = desktop
    ? `v${status.availableVersion || bundledCurrent?.version || info.version}`
    : status.availableCommit?.slice(0, 7) ||
      status.currentCommit?.slice(0, 7) ||
      bundled.version ||
      info.version
  const visibleSponsors = sponsors.filter(
    (sponsor) => (sponsorDismissals[sponsor.id] || 0) <= Date.now(),
  )
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
            desktop
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
    [desktop, overallState, resumable, t],
  )

  return (
    <div className="mx-auto flex w-full max-w-[880px] flex-col gap-3">
      <Panel className="p-5">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="flex min-w-0 items-start gap-3">
            <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <PackageCheck size={19} />
            </span>
            <span className="min-w-0">
              <strong className="block text-[16px]">
                {t('config:updateSettings.pisperAppUpdates')}
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
          <Button
            size="lg"
            disabled={checking || downloading || Boolean(componentBusy)}
            onClick={downloaded ? install : available || resumable ? download : check}
          >
            {checking || downloading ? (
              <RefreshCw className="animate-spin" size={14} />
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
                      : desktop
                        ? t('config:updateSettings.viewRelease')
                        : t('config:updateSettings.viewSourceUpdates')
                    : t('config:updateSettings.checkForUpdates')}
          </Button>
          <Button variant="outline" size="lg" className="bg-surface-subtle" onClick={openReleases}>
            <ExternalLink size={14} />
            {desktop ? 'GitHub Releases' : 'GitHub Compare'}
          </Button>
          {desktop && (
            <Button
              variant="outline"
              size="lg"
              className="bg-surface-subtle"
              onClick={openUpdateLog}
            >
              <ExternalLink size={14} />
              {t('config:updateSettings.viewUpdateDiagnosticLog')}
            </Button>
          )}
        </div>
        <div className="mt-6 border-t border-[var(--border)] pt-5">
          <SectionTitle title={t('config:updateSettings.appComponents')} />
          <div className="mt-3 divide-y divide-[var(--border)]">
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
                  className="grid min-h-[68px] items-center gap-3 py-3 sm:grid-cols-[minmax(0,1fr)_auto_auto]"
                  key={component.component}
                >
                  <div className="flex min-w-0 items-center gap-3">
                    <Icon className="shrink-0 text-[var(--text-muted)]" size={17} />
                    <div className="min-w-0">
                      <strong className="block text-[14px]">{label}</strong>
                      {component.message && (
                        <small className="mt-0.5 block text-[12px] text-[var(--text-muted)]">
                          {component.message}
                        </small>
                      )}
                    </div>
                  </div>
                  <div className="flex items-center gap-2 font-mono text-[12px]">
                    <span>v{component.currentVersion}</span>
                    {component.availableVersion &&
                      component.availableVersion !== component.currentVersion && (
                        <>
                          <ArrowRight size={12} />
                          <span>v{component.availableVersion}</span>
                        </>
                      )}
                    {component.size > 0 && component.state === 'available' && (
                      <span className="font-sans text-[var(--text-muted)]">
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
      </Panel>

      <CliSettings notify={notify} />

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
                ? `v${bundledCurrent?.version || info.version}`
                : status.currentCommit?.slice(0, 7) || t('config:updateSettings.notChecked')}
          </span>
        </div>
        {(status.releaseDate || (desktop && bundledCurrent?.date)) && (
          <small className="mt-2 block text-[12px] text-[var(--text-muted)]">
            {new Intl.DateTimeFormat(language, { dateStyle: 'long' }).format(
              new Date(status.releaseDate || bundledCurrent?.date || ''),
            )}
          </small>
        )}
        <div className="mt-4 rounded-[var(--r-sm)] border border-[var(--stroke-soft)] bg-[var(--surface-subtle)] p-4">
          <MarkdownMessage>{notes}</MarkdownMessage>
        </div>
      </Panel>

      {visibleSponsors.length > 0 && (
        <Panel className="p-5">
          <div className="flex min-w-0 items-start gap-3">
            <span className="grid w-[38px] h-[38px] [flex:0_0_auto] place-items-center rounded-[11px] bg-[var(--star-soft)] text-[var(--star-strong)]">
              <Handshake size={19} />
            </span>
            <span className="min-w-0">
              <SectionTitle title={t('config:updateSettings.sponsors')} />
              <small className="mt-1 block text-[12px] leading-5 text-[var(--text-muted)]">
                {t('config:updateSettings.sponsorsDescription')}
              </small>
            </span>
          </div>
          <div className="mt-4 divide-y divide-[var(--stroke-soft)] border-y border-[var(--stroke-soft)]">
            {visibleSponsors.map((sponsor) => (
              <div
                key={sponsor.id}
                className="grid min-h-16 grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-3 py-3"
              >
                <span className="min-w-0">
                  <span className="mb-1 flex items-center gap-2">
                    <strong className="truncate text-[14px]">{sponsor.name}</strong>
                    <Badge tone="gray">{t('config:updateSettings.sponsored')}</Badge>
                  </span>
                  <small className="block text-[12px] leading-5 text-[var(--text-muted)]">
                    {sponsor.description}
                  </small>
                </span>
                <Button asChild variant="outline" className="bg-surface-subtle">
                  <a href={sponsor.href} target="_blank" rel="noopener noreferrer sponsored">
                    <ExternalLink size={13} />
                    {t('config:updateSettings.visitSponsor')}
                  </a>
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="shrink-0"
                  aria-label={t('config:updateSettings.hideSponsor')}
                  title={t('config:updateSettings.hideSponsor')}
                  onClick={() => dismissSponsor(sponsor.id)}
                >
                  <X size={15} />
                </Button>
              </div>
            ))}
          </div>
        </Panel>
      )}
    </div>
  )
}
