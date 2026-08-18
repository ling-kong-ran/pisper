// 插件页：浏览/启停/卸载工具插件，展示启用统计与状态。
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
  AppError,
  AppEmptyState,
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

import { Button } from '@/components/ui/button'

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
    <div className="plugin-tool-config [&_label]:grid [&_label]:gap-[5px] [&_label]:text-[var(--text-muted)] [&_label]:text-[11px] [&_label]:font-[600] [&_select]:w-full [&_select]:h-[34px] [&_select]:[border:1px_solid_var(--stroke)] [&_select]:rounded-[var(--r-xs)] [&_select]:bg-[var(--solid)] [&_select]:p-[0_8px] [&_select]:text-[var(--text)] [&_select]:text-[12px] [&_input]:w-full [&_input]:h-[34px] [&_input]:[border:1px_solid_var(--stroke)] [&_input]:rounded-[var(--r-xs)] [&_input]:bg-[var(--solid)] [&_input]:p-[0_8px] [&_input]:text-[var(--text)] [&_input]:text-[12px] grid gap-[9px] [border-top:1px_solid_var(--stroke)] [padding-top:10px]">
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
      <Button
        type="button"
        variant="outline"
        size="lg"
        className="justify-self-end bg-surface-subtle"
        disabled={testing}
        onClick={onTest}
      >
        {testing ? <RefreshCw className="animate-spin" size={14} /> : <Globe2 size={14} />}
        {testing ? t('plugins:pluginsPage.testing') : t('plugins:pluginsPage.testConnection')}
      </Button>
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

  // 应用插件数据：写入列表/草稿/Web 搜索配置，并向壳层上报启用统计。
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

  // 加载插件列表（含工具启用状态）。
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

  // 保存插件策略：无改动时直接提示；否则 PUT 启停配置并刷新；防重入。
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

  // 测试 Web 搜索配置：调运行时试搜索并提示结果条数。
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

  // 卸载插件：删除前要求先保存未提交的启用策略（dirty 拦截）。
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
      <AppEmptyState>
        <RefreshCw className="animate-spin" size={24} />
        <h2>{t('plugins:pluginsPage.loadingToolPlugins')}</h2>
        {error && (
          <AppError>
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}
      </AppEmptyState>
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
    <div className="plugins-page flex min-h-[100%] flex-col gap-[12px] max-[900px]:h-auto h-full min-h-0 overflow-y-auto [scrollbar-gutter:stable]">
      <div className="plugin-toolbar max-[650px]:items-stretch max-[650px]:flex-col flex items-center justify-between gap-[12px]">
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
          <div className="plugin-presets [&_>_span]:p-[0_6px] [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] [&_button]:h-[27px] [&_button]:border-0 [&_button]:rounded-[var(--r-xs)] [&_button]:bg-transparent [&_button]:p-[0_9px] [&_button]:text-[var(--text-muted)] [&_button]:text-[12px] [&_button]:font-[600] [&_button:hover]:bg-[var(--accent-soft)] [&_button:hover]:text-[var(--star-strong)] [&_button.active]:bg-[var(--accent-soft)] [&_button.active]:text-[var(--star-strong)] max-[650px]:min-w-0 max-[650px]:overflow-x-auto flex items-center gap-[4px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--solid)] [padding:3px]">
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
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={() => requireSavedPluginState() && setInstallOpen(true)}
          >
            <PackagePlus size={15} />
            {t('plugins:pluginsPage.installPlugin')}
          </Button>
        </div>
      </div>

      <div className="plugin-catalog-heading [&_>_span]:flex [&_>_span]:items-center [&_>_span]:gap-[7px] [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[12px] [&_small]:[font-variant-numeric:tabular-nums] max-[650px]:items-start flex items-center justify-between gap-[12px] [padding:2px]">
        <span>
          <strong>{t('plugins:pluginsPage.toolCatalog')}</strong>
          <small>{filtered.length}</small>
        </span>
        <span className="plugin-catalog-summary text-[var(--text-muted)] text-[12px] [font-variant-numeric:tabular-nums] [&_>_i]:w-[7px] [&_>_i]:h-[7px] [&_>_i]:rounded-[50%] [&_>_i]:bg-[var(--status-green)] max-[650px]:flex-wrap max-[650px]:justify-end">
          <i />
          {entries.filter((tool) => tool.enabled).length}/{entries.length}{' '}
          {t('plugins:pluginsPage.enabled2')}
          {dirty && <Badge tone="amber">{t('plugins:pluginsPage.unsaved')}</Badge>}
        </span>
      </div>

      {filtered.length ? (
        <div className="plugin-tool-grid max-[650px]:grid-cols-[minmax(0,1fr)] grid grid-cols-[repeat(auto-fill,minmax(310px,1fr))] [align-items:start] gap-[10px] [padding-bottom:4px]">
          {filtered.map((tool) => {
            const open = expandedName === tool.name
            const Icon = TOOL_ICONS[tool.name] || Plug
            return (
              <article
                className="plugin-tool-card [&[data-open]]:border-[var(--stroke-hover)] [&[data-open]]:shadow-[var(--sh-1)] min-w-0 overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] bg-[var(--solid)]"
                data-open={open || undefined}
                key={tool.name}
              >
                <div className="plugin-tool-card-head hover:bg-[var(--surface-hover)] [.plugin-tool-card[data-open]_>_&]:bg-[var(--surface-hover)] max-[650px]:grid-cols-[minmax(0,1fr)_auto] max-[650px]:[padding-inline:8px] grid min-h-[56px] grid-cols-[minmax(0,1fr)_auto] items-center gap-[10px] [padding:7px_10px]">
                  <button
                    type="button"
                    className="grid min-w-0 h-full grid-cols-[32px_minmax(0,1fr)] items-center gap-[9px] border-0 bg-transparent p-0 text-inherit text-left"
                    aria-expanded={open}
                    onClick={() =>
                      setExpandedName((current) => (current === tool.name ? '' : tool.name))
                    }
                  >
                    <span className="grid w-[32px] h-[32px] place-items-center rounded-[var(--r-xs)] bg-[var(--surface-muted)] text-[var(--text-soft)]">
                      <Icon size={15} />
                    </span>
                    <span className="plugin-tool-title [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] flex min-w-0 flex-col gap-[2px]">
                      <strong title={displayToolName(tool, t)}>{displayToolName(tool, t)}</strong>
                      <small>{displayPluginName(tool, t)}</small>
                    </span>
                  </button>
                  <span className="plugin-tool-trailing [&_>_i]:w-[7px] [&_>_i]:h-[7px] [&_>_i]:rounded-[50%] [&_>_i]:bg-[var(--status-muted)] [&_>_i[data-enabled]]:bg-[var(--status-green)] max-[650px]:gap-[5px] inline-flex items-center gap-[7px]">
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
                      className="plugin-tool-chevron hover:bg-[var(--surface-muted)] hover:text-[var(--text)] [&_svg]:[transition:transform_var(--d1)_var(--ease-out)] [&[aria-expanded='true']_svg]:[transform:rotate(180deg)] grid w-[24px] h-[24px] place-items-center border-0 rounded-[var(--r-xs)] bg-transparent text-[var(--text-muted)]"
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
                  <div className="plugin-tool-details [&_>_p]:m-0 [&_>_p]:text-[var(--text-soft)] [&_>_p]:text-[12px] [&_>_p]:leading-[1.55] [&_>_code]:[overflow-wrap:anywhere] [&_>_code]:text-[var(--text)] [&_>_code]:text-[11px] [&_dl]:grid [&_dl]:grid-cols-[78px_minmax(0,1fr)] [&_dl]:gap-[6px_10px] [&_dl]:m-0 [&_dl_div]:[display:contents] [&_dt]:text-[var(--text-muted)] [&_dt]:text-[11px] [&_dd]:min-w-0 [&_dd]:m-0 [&_dd]:[overflow-wrap:anywhere] [&_dd]:text-[var(--text-soft)] [&_dd]:text-[12px] max-[650px]:[&_dl]:grid-cols-[68px_minmax(0,1fr)] grid gap-[9px] [border-top:1px_solid_var(--stroke)] bg-[var(--surface-subtle)] [padding:11px_13px_13px]">
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
                        <dd className={isHighRisk(tool.risk) ? 'text-[var(--danger)]' : ''}>
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
                      <div className="plugin-tool-warning [&_svg]:flex-none [&_svg]:mt-[1px] flex items-start gap-[7px] rounded-[var(--r-xs)] bg-[var(--warning-soft)] [padding:8px] text-[var(--warning-strong)] text-[11px] leading-[1.5]">
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
                      <Button
                        type="button"
                        variant="destructive"
                        size="lg"
                        className="[justify-self:end]"
                        onClick={() => requireSavedPluginState() && setRemoveTarget(tool)}
                      >
                        <Trash2 size={14} />
                        {t('plugins:pluginsPage.uninstallPlugin')}
                      </Button>
                    )}
                  </div>
                )}
              </article>
            )
          })}
        </div>
      ) : (
        <div className="plugin-empty [&.compact]:min-h-[90px] grid min-h-[180px] place-items-center text-[var(--text-muted)] text-[12px]">
          {t('plugins:pluginsPage.noMatchingTools')}
        </div>
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
              {removing && <LoaderCircle className="animate-spin" size={14} />}
              {t('plugins:pluginsPage.uninstallPlugin')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {error && (
        <AppError className="fixed z-[21] [right:24px] [bottom:24px] max-w-[360px] shadow-[0_12px_28px_-18px_var(--floating-shadow)]">
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
    </div>
  )
}
