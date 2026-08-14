import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  Eye,
  FileCode2,
  FolderOpen,
  Globe2,
  Image,
  LoaderCircle,
  PackagePlus,
  Plug,
  RefreshCw,
  Search,
  Server,
  ShieldAlert,
  Trash2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import {
  AppSwitch as Toggle,
  SegmentedTabs as Segmented,
  StatusBadge as Badge,
} from '@/components/ui/app-primitives'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { PluginInstallDialog } from '@/features/plugins/PluginInstallDialog'
import {
  isHighRisk,
  toolCategoryLabel,
  toolDescription,
  toolName,
  toolRiskLabel,
  toolScopeLabel,
} from '@/features/plugins/tool-labels'
import type { LucideIcon } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import type {
  InstalledPlugin,
  PluginCapability,
  PluginsData,
  WebSearchSettings,
} from '@/features/plugins/plugin-types'

type PluginsPageProps = {
  query?: string
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  onStatusChange: (status: { enabled: number; total: number }) => void
}

type SourceFilter = 'all' | 'builtin' | 'local'
type ToolEntry = PluginCapability & {
  pluginId: string
  pluginName: string
  pluginDescription: string
  pluginVersion: string
  pluginSource: 'builtin' | 'local'
  pluginBuiltIn: boolean
  pluginPermissions: string[]
  pluginSystemAccess: boolean
}

const SOURCE_FILTERS: SourceFilter[] = ['all', 'builtin', 'local']
const TOOL_ICONS: Record<string, LucideIcon> = {
  read: Eye,
  ls: FolderOpen,
  grep: Search,
  find: Search,
  edit: FileCode2,
  write: FileCode2,
  bash: Server,
  web_search: Globe2,
  browser_automation: Globe2,
  generate_visual: Image,
}

function sourceLabel(source: SourceFilter, t: ReturnType<typeof useI18n>['t']) {
  if (source === 'builtin') return t('plugins:pluginsPage.builtIn')
  if (source === 'local') return t('plugins:pluginsPage.local')
  return t('plugins:pluginsPage.all')
}

function presetLabel(preset: string, t: ReturnType<typeof useI18n>['t']) {
  if (preset === 'read-only') return t('plugins:pluginsPage.readOnly')
  if (preset === 'workspace') return t('plugins:pluginsPage.workspace')
  return t('plugins:pluginsPage.full')
}

function enabledToolNames(plugins: InstalledPlugin[]) {
  return plugins.flatMap((plugin) =>
    plugin.capabilities
      .filter((capability) => capability.enabled)
      .map((capability) => capability.name),
  )
}

function toolEntries(plugins: InstalledPlugin[]): ToolEntry[] {
  return plugins.flatMap((plugin) =>
    plugin.capabilities.map((capability) => ({
      ...capability,
      pluginId: plugin.id,
      pluginName: plugin.name,
      pluginDescription: plugin.description,
      pluginVersion: plugin.version,
      pluginSource: plugin.source,
      pluginBuiltIn: plugin.builtIn,
      pluginPermissions: plugin.permissions || [],
      pluginSystemAccess: Boolean(plugin.systemAccess),
    })),
  )
}

function sameNames(left: string[], right: string[]) {
  return [...left].sort().join('\0') === [...right].sort().join('\0')
}

function displayToolName(tool: ToolEntry, t: ReturnType<typeof useI18n>['t']) {
  return toolName({ id: tool.name, name: tool.label }, t)
}

function displayPluginName(tool: ToolEntry, t: ReturnType<typeof useI18n>['t']) {
  return tool.pluginBuiltIn ? toolCategoryLabel(tool.pluginName, t) : tool.pluginName
}

function WebSearchEditor({
  value,
  testing,
  onChange,
  onTest,
}: {
  value: WebSearchSettings
  testing: boolean
  onChange: (value: WebSearchSettings) => void
  onTest: () => void
}) {
  const { t } = useI18n()
  return (
    <div className="plugin-tool-config">
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <label>
          {t('plugins:pluginsPage.defaultLanguage')}
          <AppSelect
            value={value.language}
            onChange={(event) => onChange({ ...value, language: event.target.value })}
          >
            <option value="auto">{t('plugins:pluginsPage.auto')}</option>
            <option value="zh-CN">{t('plugins:pluginsPage.simplifiedChinese')}</option>
            <option value="en-US">{t('plugins:pluginsPage.english')}</option>
          </AppSelect>
        </label>
        <label>
          {t('plugins:pluginsPage.safeSearch')}
          <AppSelect
            value={value.safeSearch}
            onChange={(event) => onChange({ ...value, safeSearch: Number(event.target.value) })}
          >
            <option value={0}>{t('plugins:pluginsPage.off')}</option>
            <option value={1}>{t('plugins:pluginsPage.moderate')}</option>
            <option value={2}>{t('plugins:pluginsPage.strict')}</option>
          </AppSelect>
        </label>
        <label>
          {t('plugins:pluginsPage.resultCount')}
          <input
            type="number"
            min="1"
            max="12"
            value={value.maxResults}
            onChange={(event) => onChange({ ...value, maxResults: Number(event.target.value) })}
          />
        </label>
      </div>
      <button type="button" className="button secondary" disabled={testing} onClick={onTest}>
        {testing ? <RefreshCw className="spin" size={14} /> : <Globe2 size={14} />}
        {testing ? t('plugins:pluginsPage.testing') : t('plugins:pluginsPage.testConnection')}
      </button>
    </div>
  )
}

export function PluginsPage({
  query = '',
  notify,
  registerPrimaryAction,
  onStatusChange,
}: PluginsPageProps) {
  const { t } = useI18n()
  const [data, setData] = useState<PluginsData | null>(null)
  const [draft, setDraft] = useState<InstalledPlugin[]>([])
  const [expandedName, setExpandedName] = useState('')
  const [sourceFilter, setSourceFilter] = useState<SourceFilter>('all')
  const [webSearch, setWebSearch] = useState<WebSearchSettings>({
    provider: 'bing',
    language: 'auto',
    safeSearch: 1,
    maxResults: 8,
  })
  const [saving, setSaving] = useState(false)
  const [testingSearch, setTestingSearch] = useState(false)
  const [installOpen, setInstallOpen] = useState(false)
  const [removeTarget, setRemoveTarget] = useState<ToolEntry | null>(null)
  const [removing, setRemoving] = useState(false)
  const [error, setError] = useState('')

  const applyData = useCallback(
    (result: PluginsData) => {
      setData(result)
      setDraft(result.plugins)
      setWebSearch(result.webSearch)
      const tools = toolEntries(result.plugins)
      onStatusChange({
        enabled: tools.filter((tool) => tool.enabled).length,
        total: tools.length,
      })
    },
    [onStatusChange],
  )

  const load = useCallback(async () => {
    setError('')
    try {
      applyData(await apiJson<PluginsData>('/api/plugins'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }, [applyData])

  useEffect(() => {
    void load()
  }, [load])

  const entries = useMemo(() => toolEntries(draft), [draft])
  const draftEnabled = useMemo(() => enabledToolNames(draft), [draft])
  const dirty = data
    ? !sameNames(draftEnabled, data.enabledTools) ||
      JSON.stringify(webSearch) !== JSON.stringify(data.webSearch)
    : false

  const save = useCallback(async () => {
    if (saving) return
    if (!data || !dirty) {
      if (data) notify(t('plugins:pluginsPage.noPluginPolicyChanges'))
      return
    }
    setSaving(true)
    setError('')
    try {
      const updated = await apiJson<PluginsData>('/api/plugins', {
        method: 'PUT',
        body: JSON.stringify({ enabledTools: draftEnabled, webSearch }),
      })
      applyData(updated)
      notify(t('plugins:pluginsPage.pluginPolicySavedAgentRuntimeUpdated'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }, [applyData, data, dirty, draftEnabled, notify, saving, t, webSearch])

  usePagePrimaryAction(registerPrimaryAction, save)

  const testWebSearch = async () => {
    setTestingSearch(true)
    setError('')
    try {
      const result = await apiJson<{ count: number }>('/api/plugins/web-search/test', {
        method: 'POST',
        body: JSON.stringify(webSearch),
      })
      notify(
        t('plugins:pluginsPage.bingSearchIsAvailableAndReturnedCountResults', {
          count: result.count,
        }),
      )
    } catch (caught) {
      const message = caught instanceof Error ? caught.message : String(caught)
      setError(message)
      notify(message, 'error')
    } finally {
      setTestingSearch(false)
    }
  }

  const toggleCapability = (pluginId: string, capabilityName: string, enabled: boolean) => {
    setDraft((current) =>
      current.map((plugin) =>
        plugin.id === pluginId
          ? {
              ...plugin,
              enabled:
                enabled ||
                plugin.capabilities.some(
                  (capability) => capability.name !== capabilityName && capability.enabled,
                ),
              capabilities: plugin.capabilities.map((capability) =>
                capability.name === capabilityName ? { ...capability, enabled } : capability,
              ),
            }
          : plugin,
      ),
    )
  }

  const applyPreset = (preset: string) => {
    const enabled = new Set(data?.presets?.[preset] || [])
    setDraft((current) =>
      current.map((plugin) =>
        plugin.builtIn
          ? {
              ...plugin,
              enabled: plugin.capabilities.some((capability) => enabled.has(capability.name)),
              capabilities: plugin.capabilities.map((capability) => ({
                ...capability,
                enabled: enabled.has(capability.name),
              })),
            }
          : plugin,
      ),
    )
  }

  const requireSavedPluginState = () => {
    if (!dirty) return true
    setError(t('plugins:pluginsPage.saveBeforePluginChanges'))
    return false
  }

  const uninstall = async () => {
    if (!removeTarget || removing) return
    setRemoving(true)
    setError('')
    try {
      applyData(
        await apiJson<PluginsData>(`/api/plugins/${encodeURIComponent(removeTarget.pluginId)}`, {
          method: 'DELETE',
        }),
      )
      notify(t('plugins:pluginsPage.pluginUninstalled', { name: removeTarget.pluginName }))
      setExpandedName('')
      setRemoveTarget(null)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setRemoving(false)
    }
  }

  if (!data) {
    return (
      <div className="empty-state">
        <RefreshCw className="spin" size={24} />
        <h2>{t('plugins:pluginsPage.loadingToolPlugins')}</h2>
        {error && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
      </div>
    )
  }

  const normalizedQuery = query.trim().toLowerCase()
  const filtered = entries.filter((tool) => {
    if (sourceFilter !== 'all' && tool.pluginSource !== sourceFilter) return false
    return [
      displayToolName(tool, t),
      tool.name,
      tool.description,
      displayPluginName(tool, t),
      tool.pluginId,
    ]
      .join(' ')
      .toLowerCase()
      .includes(normalizedQuery)
  })

  return (
    <div className="plugins-page plugin-catalog-page">
      <div className="plugin-toolbar">
        <Segmented
          options={SOURCE_FILTERS.map((source) => sourceLabel(source, t))}
          value={sourceLabel(sourceFilter, t)}
          onChange={(label) =>
            setSourceFilter(
              SOURCE_FILTERS.find((source) => sourceLabel(source, t) === label) || 'all',
            )
          }
        />
        <div className="flex flex-wrap items-center justify-end gap-2">
          <div className="plugin-presets">
            <span>{t('plugins:pluginsPage.presets')}</span>
            {['read-only', 'workspace', 'full'].map((preset) => (
              <button
                type="button"
                key={preset}
                className={data.preset === preset && !dirty ? 'active' : ''}
                onClick={() => applyPreset(preset)}
              >
                {presetLabel(preset, t)}
              </button>
            ))}
          </div>
          <button
            type="button"
            className="button secondary"
            onClick={() => requireSavedPluginState() && setInstallOpen(true)}
          >
            <PackagePlus size={15} />
            {t('plugins:pluginsPage.installPlugin')}
          </button>
        </div>
      </div>

      <div className="plugin-catalog-heading">
        <span>
          <strong>{t('plugins:pluginsPage.toolCatalog')}</strong>
          <small>{filtered.length}</small>
        </span>
        <span className="plugin-catalog-summary">
          <i />
          {entries.filter((tool) => tool.enabled).length}/{entries.length}{' '}
          {t('plugins:pluginsPage.enabled2')}
          {dirty && <Badge tone="amber">{t('plugins:pluginsPage.unsaved')}</Badge>}
        </span>
      </div>

      {filtered.length ? (
        <div className="plugin-tool-grid">
          {filtered.map((tool) => {
            const open = expandedName === tool.name
            const Icon = TOOL_ICONS[tool.name] || Plug
            return (
              <article className="plugin-tool-card" data-open={open || undefined} key={tool.name}>
                <div className="plugin-tool-card-head">
                  <button
                    type="button"
                    className="plugin-tool-disclosure"
                    aria-expanded={open}
                    onClick={() =>
                      setExpandedName((current) => (current === tool.name ? '' : tool.name))
                    }
                  >
                    <span className="plugin-tool-icon">
                      <Icon size={15} />
                    </span>
                    <span className="plugin-tool-title">
                      <strong title={displayToolName(tool, t)}>{displayToolName(tool, t)}</strong>
                      <small>{displayPluginName(tool, t)}</small>
                    </span>
                  </button>
                  <span className="plugin-tool-trailing">
                    <i data-enabled={tool.enabled || undefined} />
                    {!tool.pluginBuiltIn && (
                      <Badge tone="amber">{t('plugins:pluginsPage.local')}</Badge>
                    )}
                    <Toggle
                      value={tool.enabled}
                      onChange={(enabled) => toggleCapability(tool.pluginId, tool.name, enabled)}
                    />
                    <button
                      type="button"
                      className="plugin-tool-chevron"
                      aria-label={t('plugins:pluginsPage.toggleToolDetails', {
                        name: displayToolName(tool, t),
                      })}
                      aria-expanded={open}
                      onClick={() =>
                        setExpandedName((current) => (current === tool.name ? '' : tool.name))
                      }
                    >
                      <ChevronDown size={14} />
                    </button>
                  </span>
                </div>

                {open && (
                  <div className="plugin-tool-details">
                    <p>{toolDescription({ id: tool.name, description: tool.description }, t)}</p>
                    <code>{tool.name}</code>
                    <dl>
                      <div>
                        <dt>{t('plugins:pluginsPage.source')}</dt>
                        <dd>
                          {displayPluginName(tool, t)}
                          {!tool.pluginBuiltIn && ` · v${tool.pluginVersion}`}
                        </dd>
                      </div>
                      <div>
                        <dt>{t('plugins:pluginsPage.riskLevel')}</dt>
                        <dd className={isHighRisk(tool.risk) ? 'danger' : ''}>
                          {toolRiskLabel(tool.risk, t)}
                        </dd>
                      </div>
                      <div>
                        <dt>{t('plugins:pluginsPage.pathScope')}</dt>
                        <dd>{toolScopeLabel({ id: tool.name, scope: tool.scope }, t)}</dd>
                      </div>
                      {!tool.pluginBuiltIn && (
                        <div>
                          <dt>{t('plugins:pluginsPage.executionMode')}</dt>
                          <dd>{t('plugins:pluginsPage.fullAccessOnly')}</dd>
                        </div>
                      )}
                    </dl>

                    {!tool.pluginBuiltIn && (
                      <div className="plugin-tool-warning">
                        <ShieldAlert size={14} />
                        <span>{t('plugins:pluginsPage.localPluginSecurityWarning')}</span>
                      </div>
                    )}

                    {tool.name === 'web_search' && (
                      <WebSearchEditor
                        value={webSearch}
                        testing={testingSearch}
                        onChange={setWebSearch}
                        onTest={testWebSearch}
                      />
                    )}

                    {!tool.pluginBuiltIn && (
                      <button
                        type="button"
                        className="button danger plugin-tool-remove"
                        onClick={() => requireSavedPluginState() && setRemoveTarget(tool)}
                      >
                        <Trash2 size={14} />
                        {t('plugins:pluginsPage.uninstallPlugin')}
                      </button>
                    )}
                  </div>
                )}
              </article>
            )
          })}
        </div>
      ) : (
        <div className="plugin-empty">{t('plugins:pluginsPage.noMatchingTools')}</div>
      )}

      <PluginInstallDialog open={installOpen} onOpenChange={setInstallOpen} onInstalled={load} />
      <AlertDialog
        open={Boolean(removeTarget)}
        onOpenChange={(open) => !open && !removing && setRemoveTarget(null)}
      >
        <AlertDialogContent className="z-[220]">
          <AlertDialogHeader>
            <AlertDialogTitle>{t('plugins:pluginsPage.uninstallPlugin')}</AlertDialogTitle>
            <AlertDialogDescription>
              {t('plugins:pluginsPage.uninstallPluginDescription', {
                name: removeTarget?.pluginName || '',
              })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={removing}>{t('common:ui.cancel')}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={removing} onClick={uninstall}>
              {removing && <LoaderCircle className="spin" size={14} />}
              {t('plugins:pluginsPage.uninstallPlugin')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {error && (
        <div className="config-error floating-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
    </div>
  )
}
