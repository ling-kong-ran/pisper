import { useCallback, useEffect, useState } from 'react'
import {
  AlertTriangle,
  CircleDot,
  Eye,
  FileCode2,
  FolderOpen,
  Globe2,
  Image,
  Pencil,
  Plug,
  RefreshCw,
  Save,
  Search,
  Server,
  ShieldCheck,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
  SegmentedTabs as Segmented,
  StatusBadge as Badge,
} from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import {
  toolCapabilityLabel,
  toolCategoryLabel,
  toolDescription,
  toolName,
  toolRiskLabel,
  toolScopeLabel,
} from './tool-labels'
import type { LucideIcon } from 'lucide-react'
import type { Notify } from '@/app/route-context'
import type { EntityRecord } from '@/types/chat'

type PluginTool = EntityRecord & {
  id: string
  enabled: boolean
  risk: string
  category: string
  description: string
}
type WebSearchSettings = {
  provider: string
  language: string
  safeSearch: number
  maxResults: number
}
type PluginChange = EntityRecord & { timestamp: string; tool: string; enabled: boolean }
type PluginsData = EntityRecord & {
  tools: PluginTool[]
  webSearch: WebSearchSettings
  changes: PluginChange[]
  preset?: string
}
type PluginsPageProps = {
  query?: string
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  onStatusChange: (status: { enabled: number; total: number }) => void
}

type PluginFilter =
  'all' | 'fileSystem' | 'search' | 'terminal' | 'visual' | 'highRisk' | 'disabled'
const FILTERS: PluginFilter[] = [
  'all',
  'fileSystem',
  'search',
  'terminal',
  'visual',
  'highRisk',
  'disabled',
]

function pluginFilterLabel(filter: PluginFilter, t: ReturnType<typeof useI18n>['t']) {
  if (filter === 'fileSystem') return t('plugins:toolLabels.fileSystem')
  if (filter === 'search') return t('plugins:toolLabels.search')
  if (filter === 'terminal') return t('plugins:toolLabels.terminal')
  if (filter === 'visual') return t('plugins:toolLabels.visual')
  if (filter === 'highRisk') return t('plugins:toolLabels.highRisk')
  if (filter === 'disabled') return t('plugins:pluginsPage.disabled')
  return t('plugins:pluginsPage.all')
}
const PRESETS: Record<string, string[]> = {
  'read-only': [
    'read',
    'grep',
    'find',
    'ls',
    'web_search',
    'browser_automation',
    'memory_search',
    'memory_remember',
    'mcp_list',
    'mcp_manage',
  ],
  workspace: [
    'read',
    'grep',
    'find',
    'ls',
    'edit',
    'write',
    'web_search',
    'browser_automation',
    'memory_search',
    'memory_remember',
    'mcp_list',
    'mcp_manage',
  ],
  full: [
    'read',
    'grep',
    'find',
    'ls',
    'edit',
    'write',
    'bash',
    'web_search',
    'browser_automation',
    'generate_visual',
    'memory_search',
    'memory_remember',
    'mcp_list',
    'mcp_manage',
  ],
}
const TOOL_ICONS: Record<string, LucideIcon> = {
  read: Eye,
  ls: FolderOpen,
  grep: Search,
  find: Search,
  edit: Pencil,
  write: FileCode2,
  bash: Server,
  web_search: Globe2,
  generate_visual: Image,
}

function pluginStatus(tools: PluginTool[]) {
  return { enabled: tools.filter((tool) => tool.enabled).length, total: tools.length }
}

export function PluginsPage({
  query = '',
  notify,
  registerPrimaryAction,
  onStatusChange,
}: PluginsPageProps) {
  const { t, language } = useI18n()
  const [data, setData] = useState<PluginsData | null>(null)
  const [draft, setDraft] = useState<PluginTool[]>([])
  const [selectedId, setSelectedId] = useState('read')
  const [tab, setTab] = useState<PluginFilter>('all')
  const [saving, setSaving] = useState(false)
  const [testingSearch, setTestingSearch] = useState(false)
  const [webSearch, setWebSearch] = useState({
    provider: 'bing',
    language: 'auto',
    safeSearch: 1,
    maxResults: 8,
  })
  const [error, setError] = useState('')

  useEffect(() => {
    apiJson<PluginsData>('/api/plugins')
      .then((result) => {
        setData(result)
        setDraft(result.tools)
        setWebSearch(result.webSearch)
        setSelectedId(result.tools[0]?.id || '')
        onStatusChange(pluginStatus(result.tools))
      })
      .catch((caught) => setError(caught instanceof Error ? caught.message : String(caught)))
  }, [onStatusChange])

  const dirty = data
    ? draft.some(
        (tool) => tool.enabled !== data.tools.find((item) => item.id === tool.id)?.enabled,
      ) || JSON.stringify(webSearch) !== JSON.stringify(data.webSearch)
    : false
  const save = useCallback(async () => {
    if (!data || !dirty) {
      if (data) notify(t('plugins:pluginsPage.noPluginPolicyChanges'))
      return
    }
    setSaving(true)
    setError('')
    try {
      const updated = await apiJson<PluginsData>('/api/plugins', {
        method: 'PUT',
        body: JSON.stringify({
          enabledTools: draft.filter((tool) => tool.enabled).map((tool) => tool.id),
          webSearch,
        }),
      })
      setData(updated)
      setDraft(updated.tools)
      setWebSearch(updated.webSearch)
      onStatusChange(pluginStatus(updated.tools))
      notify(t('plugins:pluginsPage.pluginPolicySavedAgentRuntimeUpdated'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }, [data, dirty, draft, notify, onStatusChange, t, webSearch])

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

  usePagePrimaryAction(registerPrimaryAction, save)

  if (!data)
    return (
      <Panel className="empty-state">
        <RefreshCw className="spin" size={24} />
        <h2>{t('plugins:pluginsPage.loadingToolPlugins')}</h2>
        <p>{t('plugins:pluginsPage.readingTheAgentSRegisteredToolsAndPermissions')}</p>
        {error && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
      </Panel>
    )

  const selected =
    draft.find((tool) => tool.id === selectedId) ||
    draft[0] ||
    ({ id: '', enabled: false, risk: '', category: '', description: '' } as PluginTool)
  const filtered = draft.filter((tool) => {
    if (tab === 'highRisk' && tool.risk !== 'high' && tool.risk !== '高风险') return false
    if (tab === 'disabled' && tool.enabled) return false
    const categoryByFilter: Partial<Record<PluginFilter, string[]>> = {
      fileSystem: ['filesystem', '文件系统'],
      search: ['search', '搜索'],
      terminal: ['terminal', '终端'],
      visual: ['visual', '视觉'],
    }
    if (categoryByFilter[tab] && !categoryByFilter[tab]?.includes(tool.category)) return false
    return `${toolName(tool, t)} ${tool.id} ${toolDescription(tool, t)}`
      .toLowerCase()
      .includes(query.toLowerCase())
  })
  const applyPreset = (preset: string) => {
    const tools = PRESETS[preset]
    setDraft((current) => current.map((tool) => ({ ...tool, enabled: tools.includes(tool.id) })))
  }
  const enabledHighRisk = draft.filter(
    (tool) => tool.enabled && (tool.risk === 'high' || tool.risk === '高风险'),
  )

  return (
    <div className="plugins-page">
      <div className="plugin-toolbar">
        <Segmented
          options={FILTERS.map((item) => pluginFilterLabel(item, t))}
          value={pluginFilterLabel(tab, t)}
          onChange={(label) =>
            setTab(FILTERS.find((source) => pluginFilterLabel(source, t) === label) || 'all')
          }
        />
        <div className="plugin-presets">
          <span>{t('plugins:pluginsPage.presets')}</span>
          <button
            className={data.preset === 'read-only' && !dirty ? 'active' : ''}
            onClick={() => applyPreset('read-only')}
          >
            {t('plugins:pluginsPage.readOnly')}
          </button>
          <button
            className={data.preset === 'workspace' && !dirty ? 'active' : ''}
            onClick={() => applyPreset('workspace')}
          >
            {t('plugins:pluginsPage.workspace')}
          </button>
          <button
            className={data.preset === 'full' && !dirty ? 'active' : ''}
            onClick={() => applyPreset('full')}
          >
            {t('plugins:pluginsPage.full')}
          </button>
        </div>
      </div>
      <div className="two-one-grid plugin-layout">
        <Panel>
          <div className="card-head">
            <SectionTitle
              title={`${t('plugins:pluginsPage.toolPlugins')} · ${draft.filter((tool) => tool.enabled).length}/${draft.length}`}
            />
            {dirty && <Badge tone="amber">{t('plugins:pluginsPage.unsaved')}</Badge>}
          </div>
          {filtered.length ? (
            filtered.map((tool) => {
              const Icon = TOOL_ICONS[tool.id] || Plug
              return (
                <div
                  className={`plugin-row ${selectedId === tool.id ? 'selected' : ''}`}
                  key={tool.id}
                >
                  <button className="plugin-select" onClick={() => setSelectedId(tool.id)}>
                    <span className="list-icon">
                      <Icon size={15} />
                    </span>
                    <span>
                      <strong>
                        {toolName(tool, t)}{' '}
                        <Badge
                          tone={
                            tool.risk === 'high' || tool.risk === '高风险'
                              ? 'red'
                              : tool.risk === 'medium' || tool.risk === '中风险'
                                ? 'amber'
                                : 'green'
                          }
                        >
                          {toolRiskLabel(tool.risk, t)}
                        </Badge>
                      </strong>
                      <small>{toolDescription(tool, t)}</small>
                    </span>
                  </button>
                  <em>
                    {tool.enabled
                      ? t('plugins:pluginsPage.enabled')
                      : t('plugins:pluginsPage.disable')}
                  </em>
                  <Toggle
                    value={tool.enabled}
                    onChange={(enabled) =>
                      setDraft((current) =>
                        current.map((item) => (item.id === tool.id ? { ...item, enabled } : item)),
                      )
                    }
                  />
                </div>
              )
            })
          ) : (
            <div className="plugin-empty">{t('plugins:pluginsPage.noMatchingTools')}</div>
          )}
        </Panel>
        <div className="detail-stack">
          <Panel>
            <div className="card-head">
              <SectionTitle
                title={t('plugins:pluginsPage.namePluginPolicy', {
                  name: toolName(selected, t),
                })}
              />
              <Badge tone={selected.enabled ? 'green' : 'gray'}>
                {selected.enabled
                  ? t('plugins:pluginsPage.enabled2')
                  : t('plugins:pluginsPage.disabled')}
              </Badge>
            </div>
            <p className="muted-copy">{toolDescription(selected, t)}</p>
            {selected.id === 'web_search' && (
              <div className="mb-3 grid gap-3 rounded-[10px] border border-[var(--stroke)] bg-[var(--surface-muted)] p-3">
                <div className="flex items-center gap-3 rounded-lg border border-[var(--stroke)] bg-[var(--solid)] p-3">
                  <span className="grid size-9 shrink-0 place-items-center rounded-lg bg-[var(--surface-muted)] text-[var(--text-soft)]">
                    <Globe2 size={17} />
                  </span>
                  <span className="grid gap-0.5">
                    <strong className="text-[13px] text-[var(--text)]">Bing</strong>
                    <small className="text-[12px] text-[var(--text-muted)]">
                      {t(
                        'plugins:pluginsPage.noInstallationOrAPIKeyRequiredTheAgentCanUseItAfterSaving',
                      )}
                    </small>
                  </span>
                </div>
                <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
                  <label className="grid gap-1.5 text-[12px] font-semibold text-[var(--text-soft)]">
                    {t('plugins:pluginsPage.defaultLanguage')}
                    <AppSelect
                      className="h-10 rounded-lg border border-[var(--stroke)] bg-[var(--solid)] px-2 text-[13px] text-[var(--text)]"
                      value={webSearch.language}
                      onChange={(event) =>
                        setWebSearch((current) => ({ ...current, language: event.target.value }))
                      }
                    >
                      <option value="auto">{t('plugins:pluginsPage.auto')}</option>
                      <option value="zh-CN">{t('plugins:pluginsPage.simplifiedChinese')}</option>
                      <option value="en-US">{t('plugins:pluginsPage.english')}</option>
                    </AppSelect>
                  </label>
                  <label className="grid gap-1.5 text-[12px] font-semibold text-[var(--text-soft)]">
                    {t('plugins:pluginsPage.safeSearch')}
                    <AppSelect
                      className="h-10 rounded-lg border border-[var(--stroke)] bg-[var(--solid)] px-2 text-[13px] text-[var(--text)]"
                      value={webSearch.safeSearch}
                      onChange={(event) =>
                        setWebSearch((current) => ({
                          ...current,
                          safeSearch: Number(event.target.value),
                        }))
                      }
                    >
                      <option value={0}>{t('plugins:pluginsPage.off')}</option>
                      <option value={1}>{t('plugins:pluginsPage.moderate')}</option>
                      <option value={2}>{t('plugins:pluginsPage.strict')}</option>
                    </AppSelect>
                  </label>
                  <label className="grid gap-1.5 text-[12px] font-semibold text-[var(--text-soft)]">
                    {t('plugins:pluginsPage.resultCount')}
                    <input
                      type="number"
                      min="1"
                      max="12"
                      className="h-10 rounded-lg border border-[var(--stroke)] bg-[var(--solid)] px-2 text-[13px] text-[var(--text)]"
                      value={webSearch.maxResults}
                      onChange={(event) =>
                        setWebSearch((current) => ({
                          ...current,
                          maxResults: Number(event.target.value),
                        }))
                      }
                    />
                  </label>
                </div>
                <div className="flex items-center justify-between gap-3">
                  <span className="text-[12px] leading-5 text-[var(--text-muted)]">
                    {t('plugins:pluginsPage.queriesAreSentToBingDoNotIncludeSecretsOrPrivateData')}
                  </span>
                  <button
                    type="button"
                    className="button secondary shrink-0"
                    disabled={testingSearch}
                    onClick={testWebSearch}
                  >
                    {testingSearch ? (
                      <RefreshCw className="spin" size={14} />
                    ) : (
                      <Globe2 size={14} />
                    )}
                    {testingSearch
                      ? t('plugins:pluginsPage.testing')
                      : t('plugins:pluginsPage.testConnection')}
                  </button>
                </div>
              </div>
            )}
            {[
              [t('plugins:pluginsPage.toolID'), selected.id],
              [
                t('plugins:pluginsPage.source'),
                selected.source === 'app'
                  ? t('plugins:pluginsPage.pisperAppTools')
                  : t('plugins:pluginsPage.pisperBuiltInTools'),
              ],
              [t('plugins:pluginsPage.category'), toolCategoryLabel(selected.category, t)],
              [t('plugins:pluginsPage.riskLevel'), toolRiskLabel(selected.risk, t)],
              [t('plugins:pluginsPage.pathScope'), toolScopeLabel(selected, t)],
              [t('plugins:pluginsPage.capabilities'), toolCapabilityLabel(selected, t)],
              [
                t('plugins:pluginsPage.takesEffect'),
                t('plugins:pluginsPage.onTheNextAgentRequestAfterSaving'),
              ],
            ].map((row) => (
              <div className="key-value" key={row[0]}>
                <span>{row[0]}</span>
                <strong className={row[1] === t('plugins:pluginsPage.highRisk') ? 'danger' : ''}>
                  {row[1]}
                </strong>
              </div>
            ))}
            <button
              className={`button wide ${selected.enabled ? 'danger' : 'primary'}`}
              onClick={() =>
                setDraft((current) =>
                  current.map((item) =>
                    item.id === selected.id ? { ...item, enabled: !item.enabled } : item,
                  ),
                )
              }
            >
              {selected.enabled
                ? t('plugins:pluginsPage.disableThisTool')
                : t('plugins:pluginsPage.enableThisTool')}
            </button>
          </Panel>
          <Panel>
            <SectionTitle title={t('plugins:pluginsPage.recentChanges')} />
            {data.changes.length ? (
              data.changes.slice(0, 6).map((change, index) => (
                <div className="activity-row" key={`${change.timestamp}-${change.tool}-${index}`}>
                  <CircleDot size={14} />
                  <span>
                    <strong>
                      {change.enabled
                        ? t('plugins:pluginsPage.enabled')
                        : t('plugins:pluginsPage.disable')}{' '}
                      {toolName({ id: change.tool, name: change.name }, t)}
                    </strong>
                    <small>{relativeTime(change.timestamp, language)}</small>
                  </span>
                </div>
              ))
            ) : (
              <div className="plugin-empty compact">
                {t('plugins:pluginsPage.noPermissionChangesYet')}
              </div>
            )}
          </Panel>
          <div className={`security-summary ${enabledHighRisk.length ? 'warning' : ''}`}>
            <ShieldCheck size={18} />
            <div>
              <strong>{t('plugins:pluginsPage.securitySummary')}</strong>
              <p>
                {enabledHighRisk.length
                  ? t(
                      'plugins:pluginsPage.highRiskToolsEnabledFullAccessUsesCurrentUserPermissions',
                      {
                        count: enabledHighRisk.length,
                        tools: enabledHighRisk
                          .map((tool) => toolName(tool, t))
                          .join(language === 'en-US' ? ', ' : '、'),
                      },
                    )
                  : t('plugins:pluginsPage.noHighRiskToolsAreEnabledTheAgentCanOnlyReadAndSearch')}
              </p>
            </div>
          </div>
        </div>
      </div>
      <button className="floating-save" disabled={!dirty || saving} onClick={save}>
        {saving ? <RefreshCw className="spin" size={15} /> : <Save size={15} />}
        {saving
          ? t('plugins:pluginsPage.saving')
          : dirty
            ? t('plugins:pluginsPage.savePolicy')
            : t('plugins:pluginsPage.policySaved')}
      </button>
      {error && (
        <div className="config-error floating-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
    </div>
  )
}
