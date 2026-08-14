import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Braces,
  Download,
  Package,
  RefreshCw,
  RotateCw,
  Trash2,
  Zap,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
  SegmentedTabs as Segmented,
  StatusBadge as Badge,
} from '@/components/ui/app-primitives'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'

type ExtensionCapabilities = {
  tools: string[]
  commands: string[]
  events: string[]
  flags: string[]
  shortcuts: string[]
  renderers: string[]
  providers: string[]
}

type ExtensionItem = {
  id: string
  name: string
  path: string
  enabled: boolean
  loaded: boolean
  scope: 'user' | 'project' | 'temporary'
  origin: 'package' | 'top-level'
  source: string
  diagnosticCount: number
  capabilities: ExtensionCapabilities
}

type ExtensionPackage = {
  id: string
  source: string
  scope: 'user' | 'project'
  filtered: boolean
  installed: boolean
  extensionCount: number
}

type ExtensionDiagnostic = {
  type: 'error' | 'warning' | 'collision'
  phase: 'load' | 'registration' | 'event'
  message: string
  path: string
  event?: string
  sessionId?: string
  timestamp?: string
}

type ExtensionsData = {
  cwd: string
  trusted: boolean
  locations: { global: string; project: string }
  extensions: ExtensionItem[]
  packages: ExtensionPackage[]
  diagnostics: ExtensionDiagnostic[]
  counts: {
    total: number
    enabled: number
    loaded: number
    project: number
    errors: number
    collisions: number
  }
}

type ExtensionsPageProps = {
  notify: Notify
  query?: string
  activeSessionId?: string
  registerPrimaryAction: (action: () => void) => () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}

type ExtensionFilter = 'all' | 'user' | 'project' | 'issues'
type InstallScope = 'user' | 'project'

function extensionsApiPath(path: string, activeSessionId = '') {
  if (!activeSessionId) return path
  const separator = path.includes('?') ? '&' : '?'
  return `${path}${separator}sessionId=${encodeURIComponent(activeSessionId)}`
}

export function ExtensionsPage({
  notify,
  query = '',
  activeSessionId = '',
  registerPrimaryAction,
  requestText,
  requestConfirm,
}: ExtensionsPageProps) {
  const { t } = useI18n()
  const [data, setData] = useState<ExtensionsData | null>(null)
  const [selectedId, setSelectedId] = useState('')
  const [filter, setFilter] = useState<ExtensionFilter>('all')
  const [installScope, setInstallScope] = useState<InstallScope>('user')
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')

  const load = useCallback(
    async (force = false) => {
      setError('')
      try {
        const path = force ? '/api/extensions/reload' : '/api/extensions'
        const result = await apiJson<ExtensionsData>(
          extensionsApiPath(path, activeSessionId),
          force ? { method: 'POST', body: '{}' } : undefined,
        )
        setData(result)
        setSelectedId((current) =>
          result.extensions.some((item) => item.id === current)
            ? current
            : result.extensions[0]?.id || '',
        )
        if (!result.trusted) setInstallScope('user')
        return result
      } catch (caught) {
        const message = caught instanceof Error ? caught.message : String(caught)
        setError(message)
        return null
      } finally {
        setLoading(false)
      }
    },
    [activeSessionId],
  )

  useEffect(() => {
    void load()
  }, [load])

  const installExtension = useCallback(async () => {
    if (installScope === 'project' && !data?.trusted) {
      const message = t('extensions:extensionsPage.projectTrustRequired')
      setError(message)
      notify(message, 'error')
      return
    }
    const source = await requestText?.({
      title: t('extensions:extensionsPage.installExtension'),
      message: t('extensions:extensionsPage.sourceHelp'),
      inputLabel: t('extensions:extensionsPage.source'),
      placeholder: 'E:\\path\\to\\extension, npm:@scope/package, or https://github.com/...',
      maxLength: 2_000,
      confirmLabel: t('extensions:extensionsPage.continue'),
    })
    if (!source?.trim()) return
    const approved = await requestConfirm?.({
      title: t('extensions:extensionsPage.fullAccessTitle'),
      message: t('extensions:extensionsPage.fullAccessWarning'),
      confirmLabel: t('extensions:extensionsPage.install'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<ExtensionsData>(
        extensionsApiPath('/api/extensions/install', activeSessionId),
        {
          method: 'POST',
          body: JSON.stringify({ source, scope: installScope }),
        },
      )
      setData(result)
      setSelectedId(result.extensions[0]?.id || '')
      notify(t('extensions:extensionsPage.installed'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }, [activeSessionId, data?.trusted, installScope, notify, requestConfirm, requestText, t])

  usePagePrimaryAction(registerPrimaryAction, installExtension)

  const updateExtension = async (item: ExtensionItem, enabled: boolean) => {
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<ExtensionsData>(
        extensionsApiPath(`/api/extensions/${encodeURIComponent(item.id)}`, activeSessionId),
        { method: 'PATCH', body: JSON.stringify({ enabled }) },
      )
      setData(result)
      notify(
        enabled
          ? t('extensions:extensionsPage.extensionEnabled')
          : t('extensions:extensionsPage.extensionDisabled'),
        'success',
      )
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  const updatePackage = async (item: ExtensionPackage) => {
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<ExtensionsData>(
        extensionsApiPath(
          `/api/extensions/packages/${encodeURIComponent(item.id)}/update`,
          activeSessionId,
        ),
        { method: 'POST', body: '{}' },
      )
      setData(result)
      notify(t('extensions:extensionsPage.packageUpdated'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  const removePackage = async (item: ExtensionPackage) => {
    const approved = await requestConfirm?.({
      title: t('extensions:extensionsPage.removePackage'),
      message: t('extensions:extensionsPage.removePackageWarning', { source: item.source }),
      confirmLabel: t('extensions:extensionsPage.remove'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(true)
    setError('')
    try {
      const result = await apiJson<ExtensionsData>(
        extensionsApiPath(
          `/api/extensions/packages/${encodeURIComponent(item.id)}`,
          activeSessionId,
        ),
        { method: 'DELETE' },
      )
      setData(result)
      notify(t('extensions:extensionsPage.packageRemoved'), 'success')
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setBusy(false)
    }
  }

  const filterLabels = useMemo(
    () => ({
      all: t('extensions:extensionsPage.all'),
      user: t('extensions:extensionsPage.global'),
      project: t('extensions:extensionsPage.project'),
      issues: t('extensions:extensionsPage.issues'),
    }),
    [t],
  )
  const scopeLabels = useMemo(
    () => ({
      user: t('extensions:extensionsPage.globalInstall'),
      project: t('extensions:extensionsPage.projectInstall'),
    }),
    [t],
  )

  if (loading && !data) {
    return (
      <div className="skills-page">
        <Panel className="empty-state">
          <RefreshCw className="spin" size={23} />
          <h2>{t('extensions:extensionsPage.loading')}</h2>
          <p>{t('extensions:extensionsPage.scanning')}</p>
        </Panel>
      </div>
    )
  }

  const extensions = data?.extensions || []
  const normalizedQuery = query.trim().toLowerCase()
  const filtered = extensions.filter((item) => {
    if (filter === 'user' && item.scope === 'project') return false
    if (filter === 'project' && item.scope !== 'project') return false
    if (filter === 'issues' && !item.diagnosticCount) return false
    return `${item.name} ${item.path} ${item.source}`.toLowerCase().includes(normalizedQuery)
  })
  const selected = filtered.find((item) => item.id === selectedId) || filtered[0] || null
  const selectedDiagnostics = (data?.diagnostics || []).filter(
    (item) => !selected || item.path === selected.path || !item.path,
  )
  const capabilityRows: Array<[string, string[]]> = selected
    ? [
        [t('extensions:extensionsPage.tools'), selected.capabilities.tools],
        [t('extensions:extensionsPage.commands'), selected.capabilities.commands],
        [t('extensions:extensionsPage.events'), selected.capabilities.events],
        [t('extensions:extensionsPage.providers'), selected.capabilities.providers],
        [t('extensions:extensionsPage.flags'), selected.capabilities.flags],
        [t('extensions:extensionsPage.shortcuts'), selected.capabilities.shortcuts],
        [t('extensions:extensionsPage.renderers'), selected.capabilities.renderers],
      ]
    : []

  return (
    <div className="skills-page">
      <div className="asset-toolbar">
        <Segmented
          options={(Object.keys(filterLabels) as ExtensionFilter[]).map(
            (item) => filterLabels[item],
          )}
          value={filterLabels[filter]}
          onChange={(label) =>
            setFilter(
              (Object.keys(filterLabels) as ExtensionFilter[]).find(
                (item) => filterLabels[item] === label,
              ) || 'all',
            )
          }
        />
        <Segmented
          compact
          options={(Object.keys(scopeLabels) as InstallScope[]).map((item) => scopeLabels[item])}
          value={scopeLabels[installScope]}
          onChange={(label) =>
            setInstallScope(
              (Object.keys(scopeLabels) as InstallScope[]).find(
                (item) => scopeLabels[item] === label,
              ) || 'user',
            )
          }
        />
      </div>
      {error && (
        <div className="config-error">
          <AlertTriangle size={15} />
          <span>{error}</span>
        </div>
      )}
      {data && !data.trusted && (
        <div className="permission-note">
          <AlertTriangle size={15} />
          <span>
            <strong>{t('extensions:extensionsPage.projectRestricted')}</strong>
            <small>{t('extensions:extensionsPage.projectTrustRequired')}</small>
          </span>
        </div>
      )}
      <div className="skills-layout">
        <Panel className="skill-scopes-panel">
          <div className="card-head">
            <SectionTitle title={t('extensions:extensionsPage.extensions')} />
            <Badge tone={data?.counts.errors || data?.counts.collisions ? 'amber' : 'green'}>
              {t('extensions:extensionsPage.enabledCount', {
                enabled: data?.counts.enabled || 0,
                total: data?.counts.total || 0,
              })}
            </Badge>
          </div>
          {filtered.length ? (
            filtered.map((item) => (
              <div
                className={`skill-row ${selected?.id === item.id ? 'selected' : ''}`}
                key={item.id}
              >
                <button className="skill-row-main" onClick={() => setSelectedId(item.id)}>
                  <span className="list-icon">
                    <Braces size={15} />
                  </span>
                  <span>
                    <strong>{item.name}</strong>
                    <small title={item.path}>
                      {item.scope === 'project'
                        ? t('extensions:extensionsPage.project')
                        : t('extensions:extensionsPage.global')}{' '}
                      · {item.source}
                    </small>
                  </span>
                </button>
                <Toggle
                  value={item.enabled}
                  disabled={busy || (item.scope === 'project' && !data?.trusted)}
                  ariaLabel={t('extensions:extensionsPage.toggleExtension', { name: item.name })}
                  onChange={(enabled) => void updateExtension(item, enabled)}
                />
              </div>
            ))
          ) : (
            <p className="muted-copy skills-empty-copy">
              {t('extensions:extensionsPage.noExtensions')}
            </p>
          )}
        </Panel>
        <div className="detail-stack">
          <Panel>
            <div className="card-head">
              <SectionTitle title={t('extensions:extensionsPage.packages')} />
              <Badge tone="gray">{data?.packages.length || 0}</Badge>
            </div>
            {(data?.packages || []).length ? (
              data?.packages.map((item) => (
                <div className="market-row" key={item.id}>
                  <span className="list-icon">
                    <Package size={15} />
                  </span>
                  <span>
                    <strong title={item.source}>{item.source}</strong>
                    <small>
                      {item.scope === 'project'
                        ? t('extensions:extensionsPage.project')
                        : t('extensions:extensionsPage.global')}{' '}
                      ·{' '}
                      {t('extensions:extensionsPage.extensionCount', {
                        count: item.extensionCount,
                      })}
                    </small>
                  </span>
                  <span className="workflow-row-actions">
                    <button
                      className="icon-button"
                      title={t('extensions:extensionsPage.updatePackage')}
                      disabled={busy || (item.scope === 'project' && !data?.trusted)}
                      onClick={() => void updatePackage(item)}
                    >
                      <RotateCw size={14} />
                    </button>
                    <button
                      className="icon-button danger"
                      title={t('extensions:extensionsPage.removePackage')}
                      disabled={busy || (item.scope === 'project' && !data?.trusted)}
                      onClick={() => void removePackage(item)}
                    >
                      <Trash2 size={14} />
                    </button>
                  </span>
                </div>
              ))
            ) : (
              <p className="muted-copy skills-empty-copy">
                {t('extensions:extensionsPage.noPackages')}
              </p>
            )}
          </Panel>
          <Panel>
            <div className="card-head">
              <SectionTitle title={t('extensions:extensionsPage.diagnostics')} />
              <button
                className="icon-button"
                title={t('extensions:extensionsPage.reload')}
                disabled={busy}
                onClick={() => void load(true)}
              >
                <RefreshCw size={14} />
              </button>
            </div>
            {(data?.diagnostics || []).length ? (
              data?.diagnostics.map((item, index) => (
                <div
                  className={item.type === 'error' ? 'config-error' : 'permission-note'}
                  key={`${item.phase}-${item.path}-${index}`}
                  title={item.path}
                >
                  <AlertTriangle size={14} />
                  <span>
                    <strong>
                      {item.type === 'collision'
                        ? t('extensions:extensionsPage.collision')
                        : item.type === 'warning'
                          ? t('extensions:extensionsPage.warning')
                          : item.phase === 'event'
                            ? t('extensions:extensionsPage.eventError')
                            : t('extensions:extensionsPage.loadError')}
                    </strong>
                    {item.message}
                  </span>
                </div>
              ))
            ) : (
              <p className="muted-copy skills-empty-copy">
                {t('extensions:extensionsPage.noDiagnostics')}
              </p>
            )}
          </Panel>
        </div>
        <div className="detail-stack">
          <Panel>
            <SectionTitle title={t('extensions:extensionsPage.selectedExtension')} />
            <h2>{selected?.name || t('extensions:extensionsPage.noSelection')}</h2>
            <p className="muted-copy" title={selected?.path}>
              {selected?.path || t('extensions:extensionsPage.selectExtension')}
            </p>
            {[
              [
                t('extensions:extensionsPage.status'),
                selected?.loaded
                  ? t('extensions:extensionsPage.loaded')
                  : selected?.enabled
                    ? t('extensions:extensionsPage.loadFailed')
                    : t('extensions:extensionsPage.disabled'),
              ],
              [
                t('extensions:extensionsPage.scope'),
                selected?.scope === 'project'
                  ? t('extensions:extensionsPage.project')
                  : t('extensions:extensionsPage.global'),
              ],
              [
                t('extensions:extensionsPage.runtimeAccess'),
                t('extensions:extensionsPage.fullSystemAccess'),
              ],
              [t('extensions:extensionsPage.source'), selected?.source || '—'],
            ].map(([label, value]) => (
              <div className="key-value" key={label}>
                <span>{label}</span>
                <strong>{value}</strong>
              </div>
            ))}
            <button
              className="button primary wide"
              disabled={busy || (!data?.trusted && installScope === 'project')}
              onClick={() => void installExtension()}
            >
              <Download size={14} />
              {t('extensions:extensionsPage.installExtension')}
            </button>
          </Panel>
          <Panel>
            <SectionTitle title={t('extensions:extensionsPage.registrations')} />
            {capabilityRows.map(([label, values]) => (
              <div className="key-value" key={label}>
                <span>{label}</span>
                <strong title={values.join(', ')}>
                  {values.length ? values.join(', ') : t('extensions:extensionsPage.none')}
                </strong>
              </div>
            ))}
            {selected && selectedDiagnostics.length > 0 && (
              <div className="permission-note">
                <Zap size={15} />
                <span>
                  <strong>
                    {t('extensions:extensionsPage.issueCount', {
                      count: selectedDiagnostics.length,
                    })}
                  </strong>
                  <small>{selectedDiagnostics[0]?.message}</small>
                </span>
              </div>
            )}
          </Panel>
        </div>
      </div>
    </div>
  )
}
