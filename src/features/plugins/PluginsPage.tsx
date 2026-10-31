// 插件页（功能 › 扩展 › 插件）：浏览 / 启停 / 卸载工具插件，配置 Web 搜索，安装 Pi Extension。
// 外形用功能页模板（规格 A6 第 1 批）：工具行放搜索、来源筛选与主操作“保存策略”，工具与扩展都是列表行，
// 行内展开详情；数据流（草稿 → 保存策略、卸载前先保存）与改版前相同。
import {
  FeatureAlert,
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Eye,
  ExternalLink,
  FileCode2,
  FolderOpen,
  Globe2,
  Image,
  Package,
  PackagePlus,
  Plug,
  Save,
  Search,
  Server,
  ShieldAlert,
  Trash2,
} from 'lucide-react'
import { AppSelect } from '@/components/AppSelect'
import {
  AppSwitch as Toggle,
  SegmentedTabs as Segmented,
  AppError,
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
import type { Notify } from '@/app/route-context'
import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import { apiJson } from '@/lib/api'
import { useShortcutLabel } from '@/lib/shortcuts'
import type { ConfirmDialogOptions, PromptDialogOptions } from '@/hooks/useAppDialog'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { runtimeFeatureAvailable } from '@/types/runtime-capabilities'
import { PluginInstallDialog } from '@/features/plugins/PluginInstallDialog'
import {
  isHighRisk,
  isMediumRisk,
  toolCategoryLabel,
  toolDescription,
  toolName,
  toolRiskLabel,
  toolScopeLabel,
} from '@/features/plugins/tool-labels'
import type { LucideIcon } from 'lucide-react'
import type { EntityRecord } from '@/types/chat'
import type {
  InstalledPlugin,
  PluginCapability,
  PluginsData,
  WebSearchSettings,
} from '@/features/plugins/plugin-types'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { OrbitSpinner } from '@/components/ui/spinner'
import { cn } from '@/lib/utils'

type PluginsPageProps = {
  notify: Notify
  // 从功能进入时由应用壳提供：标题上方的“‹ 功能”。
  onBack?: () => void
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}

type ExtensionResourceType = 'tool' | 'prompt' | 'skill'

const EXTENSION_RESOURCE_TYPES: ExtensionResourceType[] = ['tool', 'prompt', 'skill']

function extensionResourceTypes(item: EntityRecord): ExtensionResourceType[] {
  const types = Array.isArray(item.resourceTypes)
    ? item.resourceTypes.filter((type): type is ExtensionResourceType =>
        EXTENSION_RESOURCE_TYPES.includes(type as ExtensionResourceType),
      )
    : []
  return types.length ? types : ['tool']
}

function extensionResourceLabel(type: ExtensionResourceType, t: ReturnType<typeof useI18n>['t']) {
  const labels: Record<ExtensionResourceType, string> = {
    tool: t('plugins:pluginsPage.extensionType.tool'),
    prompt: t('plugins:pluginsPage.extensionType.prompt'),
    skill: t('plugins:pluginsPage.extensionType.skill'),
  }
  return labels[type]
}

type SourceFilter = 'all' | 'builtin' | 'local'
type ToolEntry = PluginCapability & {
  pluginId: string
  pluginName: string
  pluginDescription: string
  pluginVersion: string
  pluginSource: 'builtin' | 'local' | 'pi'
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

function enabledToolNames(plugins: InstalledPlugin[]) {
  return plugins.flatMap((plugin) =>
    plugin.managedExternally
      ? []
      : plugin.capabilities
          .filter((capability) => capability.enabled)
          .map((capability) => capability.name),
  )
}

function toolEntries(plugins: InstalledPlugin[]): ToolEntry[] {
  return plugins
    .filter((plugin) => !plugin.managedExternally || plugin.source === 'pi')
    .flatMap((plugin) =>
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

function computerUseEnabled(plugins: InstalledPlugin[]) {
  const plugin = plugins.find(
    (candidate) => candidate.source === 'pi' && candidate.name === 'Computer Use',
  )
  return plugin?.enabled !== false
}

function piExtensionSettings(plugins: InstalledPlugin[]) {
  return Object.fromEntries(
    plugins
      .filter(
        (plugin) =>
          plugin.source === 'pi' && plugin.name !== 'Computer Use' && Boolean(plugin.packageSource),
      )
      .map((plugin) => [plugin.packageSource as string, plugin.enabled !== false]),
  )
}

function sameNames(left: string[], right: string[]) {
  return [...left].sort().join('\0') === [...right].sort().join('\0')
}

function displayToolName(tool: ToolEntry, t: ReturnType<typeof useI18n>['t']) {
  return toolName({ id: tool.name, name: tool.label }, t)
}

function displayPluginName(tool: ToolEntry, t: ReturnType<typeof useI18n>['t']) {
  return tool.pluginSource === 'pi' ? tool.pluginName : toolCategoryLabel(tool.pluginName, t)
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
    <div className="plugin-tool-config grid gap-2.5 border-t border-stroke pt-3 [&_label]:grid [&_label]:gap-1.5 [&_label]:text-xs [&_label]:font-medium [&_label]:text-content-secondary">
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
          <Input
            type="number"
            min="1"
            max="12"
            className="tabular-nums"
            value={value.maxResults}
            onChange={(event) => onChange({ ...value, maxResults: Number(event.target.value) })}
          />
        </label>
      </div>
      <Button
        type="button"
        variant="secondary"
        className="justify-self-end max-[650px]:h-11"
        loading={testing}
        disabled={testing}
        onClick={onTest}
      >
        <Globe2 />
        {testing ? t('plugins:pluginsPage.testing') : t('plugins:pluginsPage.testConnection')}
      </Button>
    </div>
  )
}

function ExtensionMarket({
  query,
  notify,
  requestText,
  requestConfirm,
}: {
  query: string
  notify: Notify
  requestText?: (options?: PromptDialogOptions) => Promise<string | null>
  requestConfirm?: (options?: ConfirmDialogOptions) => Promise<boolean>
}) {
  const { t } = useI18n()
  const [marketQuery, setMarketQuery] = useState('')
  const [market, setMarket] = useState<EntityRecord | null>(null)
  const [installed, setInstalled] = useState<EntityRecord[]>([])
  const [loading, setLoading] = useState(false)
  const [busy, setBusy] = useState('')
  const [marketError, setMarketError] = useState('')

  const load = useCallback(async (name = '') => {
    setLoading(true)
    setMarketError('')
    try {
      const [catalog, extensions] = await Promise.all([
        apiJson<EntityRecord>(`/api/extensions/market?name=${encodeURIComponent(name)}`),
        apiJson<EntityRecord>('/api/extensions'),
      ])
      setMarket(catalog)
      setInstalled(extensions.packages || [])
    } catch (caught) {
      setMarketError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load('')
  }, [load])

  const visiblePackages = (market?.packages || []).filter((item: EntityRecord) =>
    `${item.name} ${item.description}`.toLowerCase().includes(query.trim().toLowerCase()),
  )

  const install = async (name: string) => {
    const approved = await requestConfirm?.({
      title: t('plugins:pluginsPage.installExtension'),
      message: t('plugins:pluginsPage.extensionSecurityWarning', { name }),
      confirmLabel: t('plugins:pluginsPage.installExtension'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(name)
    try {
      const isLocalPath = /^(?:[.~\\/]|[A-Za-z]:[\\/])/.test(name)
      const installSource =
        isLocalPath || /^(?:npm:|https?:\/\/|git\+)/i.test(name) ? name : `npm:${name}`
      const result = await apiJson<EntityRecord>('/api/extensions/install', {
        method: 'POST',
        body: JSON.stringify({ source: installSource }),
      })
      setInstalled(result.packages || [])
      notify(t('plugins:pluginsPage.extensionInstalled'), 'success')
    } catch (caught) {
      notify(caught instanceof Error ? caught.message : String(caught), 'error')
    } finally {
      setBusy('')
    }
  }

  const installCustom = async () => {
    const source = await requestText?.({
      title: t('plugins:pluginsPage.installExtension'),
      message: t('plugins:pluginsPage.extensionSourceHelp'),
      inputLabel: t('plugins:pluginsPage.extensionSource'),
      placeholder: 'npm:@scope/package or https://github.com/…',
      maxLength: 2_000,
    })
    if (!source?.trim()) return
    await install(source.trim())
  }

  const uninstall = async (item: EntityRecord) => {
    const approved = await requestConfirm?.({
      title: t('plugins:pluginsPage.uninstallExtension'),
      message: t('plugins:pluginsPage.uninstallExtensionDescription', { name: item.name }),
      confirmLabel: t('plugins:pluginsPage.uninstallExtension'),
      tone: 'danger',
    })
    if (approved === false) return
    setBusy(item.source)
    try {
      await apiJson('/api/extensions', {
        method: 'DELETE',
        body: JSON.stringify({ source: item.source, scope: item.scope }),
      })
      await load(marketQuery)
      notify(t('plugins:pluginsPage.extensionUninstalled'), 'success')
    } catch (caught) {
      notify(caught instanceof Error ? caught.message : String(caught), 'error')
    } finally {
      setBusy('')
    }
  }

  const installedGroups = EXTENSION_RESOURCE_TYPES.map((type) => ({
    type,
    items: installed.filter((item) => extensionResourceTypes(item).includes(type)),
  })).filter((group) => group.items.length > 0)

  return (
    <>
      {/* 已安装的 Extension 按资源类型分组（工具 / Prompt / Skill），每个都是一行，右侧“卸载”。 */}
      {installedGroups.map((group) => (
        <FeatureSection
          key={group.type}
          className="plugin-installed-extensions"
          title={`${t('plugins:pluginsPage.installedExtensions')} · ${extensionResourceLabel(group.type, t)}`}
          count={group.items.length}
        >
          <FeatureList>
            {group.items.map((item) => (
              <FeatureRow
                key={`${group.type}:${item.scope}:${item.source}`}
                icon={<Package />}
                name={<span className="font-mono">{item.name}</span>}
                nameMeta={`v${item.version}`}
                description={<span className="font-mono text-xs">{item.source}</span>}
                actions={
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="text-content-muted hover:bg-danger-soft hover:text-danger"
                    loading={busy === item.source}
                    disabled={Boolean(busy)}
                    onClick={() => void uninstall(item)}
                  >
                    <Trash2 />
                    {t('plugins:pluginsPage.uninstallExtension')}
                  </Button>
                }
              />
            ))}
          </FeatureList>
        </FeatureSection>
      ))}
      <FeatureSection
        className="plugin-market"
        title={t('plugins:pluginsPage.extensionMarket')}
        actions={
          <>
            <Button asChild variant="ghost" size="sm">
              <a href="https://pi.dev/packages" target="_blank" rel="noreferrer">
                <ExternalLink />
                {t('plugins:pluginsPage.openPackageCatalog')}
              </a>
            </Button>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={installCustom}
              disabled={Boolean(busy)}
            >
              <PackagePlus />
              {t('plugins:pluginsPage.installExtension')}
            </Button>
          </>
        }
      >
        <form
          className="mb-3 flex min-w-0 gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            void load(marketQuery)
          }}
        >
          <Input
            className="min-w-0 flex-1 max-[650px]:h-11"
            value={marketQuery}
            onChange={(event) => setMarketQuery(event.target.value)}
            placeholder={t('plugins:pluginsPage.searchPackageCatalog')}
            aria-label={t('plugins:pluginsPage.searchPackageCatalog')}
          />
          <Button
            type="submit"
            variant="secondary"
            className="h-[34px] max-[650px]:h-11"
            loading={loading}
            disabled={loading}
          >
            <Search />
            {t('plugins:pluginsPage.search')}
          </Button>
        </form>
        {marketError && <FeatureAlert className="mt-0 mb-3">{marketError}</FeatureAlert>}
        {visiblePackages.length > 0 && (
          // 目录结果也是列表行（不是卡片墙）：名称、说明（最多两行）、发布日期，右侧“安装 / 已安装”。
          <FeatureList>
            {visiblePackages.slice(0, 12).map((item: EntityRecord) => {
              const isInstalled = installed.some(
                (entry) => entry.name === item.name || entry.source === `npm:${item.name}`,
              )
              return (
                <FeatureRow
                  key={item.name}
                  icon={<Package />}
                  name={<span className="font-mono">{item.name}</span>}
                  description={item.description}
                  status={item.published}
                  actions={
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      loading={busy === item.name}
                      disabled={isInstalled || Boolean(busy)}
                      onClick={() => void install(item.name)}
                    >
                      {isInstalled
                        ? t('plugins:pluginsPage.installed')
                        : t('plugins:pluginsPage.installExtension')}
                    </Button>
                  }
                />
              )
            })}
          </FeatureList>
        )}
      </FeatureSection>
    </>
  )
}

export function PluginsPage({ notify, onBack, requestText, requestConfirm }: PluginsPageProps) {
  const { t } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const customPluginsAvailable = runtimeFeatureAvailable(capabilities, 'plugins')
  const sourceFilters = customPluginsAvailable ? SOURCE_FILTERS : SOURCE_FILTERS.slice(0, 2)
  const [query, setQuery] = useState('')
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
  const [retrying, setRetrying] = useState(false)

  // 应用插件数据：写入列表、草稿和 Web 搜索配置。
  const applyData = useCallback((result: PluginsData) => {
    setData(result)
    setDraft(result.plugins)
    setWebSearch(result.webSearch)
  }, [])

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
      computerUseEnabled(draft) !== data.computerUseEnabled ||
      JSON.stringify(piExtensionSettings(draft)) !== JSON.stringify(data.piExtensions) ||
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
        body: JSON.stringify({
          enabledTools: draftEnabled,
          webSearch,
          computerUseEnabled: computerUseEnabled(draft),
          piExtensions: piExtensionSettings(draft),
        }),
      })
      applyData(updated)
      notify(t('plugins:pluginsPage.pluginPolicySavedAgentRuntimeUpdated'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }, [applyData, data, dirty, draft, draftEnabled, notify, saving, t, webSearch])

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
      current.map((plugin) => {
        if (plugin.id !== pluginId) return plugin
        const isPiExtension = plugin.source === 'pi' && plugin.managedExternally
        return {
          ...plugin,
          enabled: enabled || plugin.capabilities.some((capability) => capability.enabled),
          capabilities: plugin.capabilities.map((capability) =>
            isPiExtension
              ? { ...capability, enabled }
              : capability.name === capabilityName
                ? { ...capability, enabled }
                : capability,
          ),
        }
      }),
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

  const title = t('navigation:navigation.plugins')
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
    <FeaturePage
      title={title}
      description={t('plugins:pluginsPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchPage', { page: title })}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            <>
              {customPluginsAvailable && (
                <Button
                  type="button"
                  variant="secondary"
                  className="h-9"
                  disabled={!data}
                  onClick={() => requireSavedPluginState() && setInstallOpen(true)}
                >
                  <PackagePlus />
                  {t('plugins:pluginsPage.installPlugin')}
                </Button>
              )}
              {/* 主操作（原共享页头的“保存策略”）：没有改动时点它只提示“没有变化”，与改版前一致。 */}
              <Button
                type="button"
                className="h-9"
                data-plugins-save
                loading={saving}
                disabled={!data || saving}
                onClick={() => void save()}
              >
                <Save />
                {t('plugins:pluginsPage.savePolicy')}
              </Button>
            </>
          }
          filters={
            <Segmented
              options={sourceFilters.map((source) => sourceLabel(source, t))}
              value={sourceLabel(sourceFilter, t)}
              onChange={(label) =>
                setSourceFilter(
                  sourceFilters.find((source) => sourceLabel(source, t) === label) || 'all',
                )
              }
            />
          }
        />
      }
    >
      {!data ? (
        error ? (
          <PageState
            kind="error"
            title={t('plugins:pluginsPage.loadFailed')}
            description={error}
            action={
              <Button
                variant="secondary"
                className="max-[650px]:h-11"
                loading={retrying}
                disabled={retrying}
                onClick={() => {
                  setRetrying(true)
                  void load().finally(() => setRetrying(false))
                }}
              >
                {t('common:pageTemplate.retry')}
              </Button>
            }
          />
        ) : (
          <PageState kind="loading" title={t('plugins:pluginsPage.loadingToolPlugins')} />
        )
      ) : (
        <>
          <FeatureSection
            className="plugin-catalog"
            title={t('plugins:pluginsPage.toolCatalog')}
            count={filtered.length}
            meta={
              <span className="plugin-catalog-summary">
                {entries.filter((tool) => tool.enabled).length}/{entries.length}{' '}
                {t('plugins:pluginsPage.enabled2')}
              </span>
            }
            actions={
              // 未保存是中性提示（琥珀只表示“需要你”，规格 6.1）；保存键就在工具行里。
              dirty && (
                <span
                  data-plugins-unsaved
                  className="flex items-center gap-1.5 text-xs font-medium text-content-secondary"
                >
                  <span aria-hidden="true" className="size-1.5 rounded-full bg-content-secondary" />
                  {t('plugins:pluginsPage.unsaved')}
                </span>
              )
            }
          >
            {filtered.length ? (
              <FeatureList className="plugin-tool-grid">
                {filtered.map((tool) => {
                  const Icon = TOOL_ICONS[tool.name] || Plug
                  const name = displayToolName(tool, t)
                  return (
                    <FeatureRow
                      key={tool.name}
                      className="plugin-tool-card"
                      data-plugin-tool={tool.name}
                      icon={<Icon />}
                      name={<span title={name}>{name}</span>}
                      nameMeta={tool.name}
                      // 风险等级写成文字，不用红 / 琥珀圆点（规格 6.1：状态与风险靠文字和形状）。
                      description={`${displayPluginName(tool, t)} · ${toolRiskLabel(tool.risk, t)}`}
                      status={!tool.pluginBuiltIn ? t('plugins:pluginsPage.local') : undefined}
                      actions={
                        <Toggle
                          value={tool.enabled}
                          onChange={(enabled) =>
                            toggleCapability(tool.pluginId, tool.name, enabled)
                          }
                          ariaLabel={name}
                        />
                      }
                      expanded={expandedName === tool.name}
                      onExpandedChange={(open) => setExpandedName(open ? tool.name : '')}
                      toggleLabel={t('plugins:pluginsPage.toggleToolDetails', { name })}
                      detail={
                        <div className="plugin-tool-details grid gap-3">
                          <p className="text-sm leading-relaxed text-content-soft">
                            {toolDescription({ id: tool.name, description: tool.description }, t)}
                          </p>
                          <dl className="grid grid-cols-[88px_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs [&_dd]:min-w-0 [&_dd]:[overflow-wrap:anywhere] [&_dd]:text-content-soft [&_dt]:text-content-muted [&>div]:contents">
                            <div>
                              <dt>{t('plugins:pluginsPage.source')}</dt>
                              <dd className="tabular-nums">
                                {displayPluginName(tool, t)}
                                {!tool.pluginBuiltIn && ` · v${tool.pluginVersion}`}
                              </dd>
                            </div>
                            <div>
                              <dt>{t('plugins:pluginsPage.riskLevel')}</dt>
                              <dd
                                className={cn(
                                  (isHighRisk(tool.risk) || isMediumRisk(tool.risk)) &&
                                    'font-medium text-content!',
                                )}
                              >
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
                            <div className="plugin-tool-warning flex items-start gap-2 rounded-sm bg-surface-subtle p-2.5 text-xs leading-normal text-content-secondary">
                              <ShieldAlert className="mt-px size-3.5 shrink-0" aria-hidden="true" />
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
                              variant="ghost"
                              className="justify-self-end text-danger hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                              onClick={() => requireSavedPluginState() && setRemoveTarget(tool)}
                            >
                              <Trash2 />
                              {t('plugins:pluginsPage.uninstallPlugin')}
                            </Button>
                          )}
                        </div>
                      }
                    />
                  )
                })}
              </FeatureList>
            ) : (
              <InlineEmptyState compact title={t('plugins:pluginsPage.noMatchingTools')} />
            )}
          </FeatureSection>

          <ExtensionMarket
            query={query}
            notify={notify}
            requestText={requestText}
            requestConfirm={requestConfirm}
          />
        </>
      )}

      {customPluginsAvailable && (
        <PluginInstallDialog open={installOpen} onOpenChange={setInstallOpen} onInstalled={load} />
      )}
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
              {removing && <OrbitSpinner className="text-current" />}
              {t('plugins:pluginsPage.uninstallPlugin')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {error && data && (
        <AppError className="fixed right-6 bottom-6 z-[21] max-w-[360px] border border-danger-border shadow-float">
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
    </FeaturePage>
  )
}
