import { PageFrame } from '@/components/app/PageFrame'
import { PageToolbar } from '@/components/app/PageToolbar'
import { InlineEmptyState } from '@/components/app/PageState'
// 插件页：浏览/启停/卸载工具插件，展示启用统计与状态。
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  ChevronDown,
  Eye,
  ExternalLink,
  FileCode2,
  FolderOpen,
  Globe2,
  Image,
  PackagePlus,
  Plug,
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
import type { Notify } from '@/app/route-context'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { apiJson } from '@/lib/api'
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
  query?: string
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
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

// 目录卡片：1px 描边 + r-md，无投影；卡片头位于次级表面，13/500 标题 + 等宽计数。
const CARD = 'min-w-0 overflow-hidden rounded-md border border-stroke bg-panel'
const CARD_HEAD =
  'flex min-h-11 min-w-0 items-center justify-between gap-3 border-b border-stroke-soft bg-surface-subtle px-3.5 py-1.5'
// 单色键帽图标：工具类别靠图形区分，不用彩色底。
const ICON_KEY =
  'grid size-7 shrink-0 place-items-center rounded-sm border border-key-border bg-key text-content-secondary shadow-key [&_svg]:size-3.5'

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
    <div className="plugin-tool-config grid gap-2.5 border-t border-stroke-soft pt-3 [&_label]:grid [&_label]:gap-1.5 [&_label]:text-xs [&_label]:font-medium [&_label]:text-content-secondary">
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
    <section className={cn('plugin-market mt-4', CARD)}>
      <header className={cn(CARD_HEAD, 'flex-wrap')}>
        <h2 className="text-sm font-medium text-content">
          {t('plugins:pluginsPage.extensionMarket')}
        </h2>
        <div className="flex flex-wrap items-center gap-1.5">
          <Button asChild variant="ghost" size="sm" className="max-[650px]:h-11">
            <a href="https://pi.dev/packages" target="_blank" rel="noreferrer">
              <ExternalLink />
              {t('plugins:pluginsPage.openPackageCatalog')}
            </a>
          </Button>
          <Button
            type="button"
            variant="secondary"
            size="sm"
            className="max-[650px]:h-11"
            onClick={installCustom}
            disabled={Boolean(busy)}
          >
            <PackagePlus />
            {t('plugins:pluginsPage.installExtension')}
          </Button>
        </div>
      </header>
      <div className="grid gap-3 p-3.5">
        <form
          className="flex min-w-0 gap-2"
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
        {marketError && (
          <p role="alert" className="text-xs text-danger">
            {marketError}
          </p>
        )}
        {installedGroups.length > 0 && (
          <div className="grid gap-2">
            <h3 className="text-xs font-medium tracking-[.02em] text-content-muted">
              {t('plugins:pluginsPage.installedExtensions')}
            </h3>
            {installedGroups.map((group) => (
              <section
                className="overflow-hidden rounded-md border border-stroke-soft"
                key={group.type}
              >
                <h4 className="border-b border-stroke-soft bg-surface-subtle px-3 py-1.5 text-xs font-medium text-content-secondary">
                  {extensionResourceLabel(group.type, t)}
                </h4>
                <ul className="divide-y divide-stroke-soft">
                  {group.items.map((item) => (
                    <li
                      className="flex min-w-0 items-center justify-between gap-2 py-1.5 pr-1.5 pl-3"
                      key={`${group.type}:${item.scope}:${item.source}`}
                    >
                      <span className="min-w-0">
                        <strong className="block truncate font-mono text-xs font-medium text-content">
                          {item.name}
                        </strong>
                        <small className="block truncate font-mono text-xs text-content-muted">
                          {item.source} · v{item.version}
                        </small>
                      </span>
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        className="text-content-muted hover:bg-danger-soft hover:text-danger max-[650px]:h-11"
                        loading={busy === item.source}
                        disabled={Boolean(busy)}
                        onClick={() => void uninstall(item)}
                      >
                        <Trash2 />
                        {t('plugins:pluginsPage.uninstallExtension')}
                      </Button>
                    </li>
                  ))}
                </ul>
              </section>
            ))}
          </div>
        )}
        {visiblePackages.length > 0 && (
          <ul className="grid grid-cols-[repeat(auto-fill,minmax(min(230px,100%),1fr))] gap-2">
            {visiblePackages.slice(0, 12).map((item: EntityRecord) => {
              const isInstalled = installed.some(
                (entry) => entry.name === item.name || entry.source === `npm:${item.name}`,
              )
              return (
                <li
                  className="grid min-w-0 content-start gap-1.5 rounded-md border border-stroke bg-panel p-3"
                  key={item.name}
                >
                  <strong
                    className="truncate font-mono text-xs font-medium text-content"
                    title={item.name}
                  >
                    {item.name}
                  </strong>
                  <p className="line-clamp-3 min-h-[54px] text-xs leading-[1.5] text-content-secondary">
                    {item.description}
                  </p>
                  <div className="flex items-center justify-between gap-2">
                    <small className="text-xs text-content-muted tabular-nums">
                      {item.published}
                    </small>
                    <Button
                      type="button"
                      variant="secondary"
                      size="sm"
                      className="max-[650px]:h-11"
                      loading={busy === item.name}
                      disabled={isInstalled || Boolean(busy)}
                      onClick={() => void install(item.name)}
                    >
                      {isInstalled
                        ? t('plugins:pluginsPage.installed')
                        : t('plugins:pluginsPage.installExtension')}
                    </Button>
                  </div>
                </li>
              )
            })}
          </ul>
        )}
      </div>
    </section>
  )
}

export function PluginsPage({
  query = '',
  notify,
  registerPrimaryAction,
  requestText,
  requestConfirm,
}: PluginsPageProps) {
  const { t } = useI18n()
  const capabilities = useRuntimeCapabilitiesStore((state) => state.capabilities)
  const customPluginsAvailable = runtimeFeatureAvailable(capabilities, 'plugins')
  const sourceFilters = customPluginsAvailable ? SOURCE_FILTERS : SOURCE_FILTERS.slice(0, 2)
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

  if (!data) {
    return (
      <AppEmptyState>
        <OrbitSpinner className="size-5" />
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
    <PageFrame variant="collection">
      {/* 工具栏自带 16px 下边距，这里不再叠加 gap，两张卡片之间单独留 16px。 */}
      <div className="flex min-w-0 flex-col">
        <PageToolbar
          actions={
            customPluginsAvailable && (
              <Button
                type="button"
                variant="secondary"
                className="max-[650px]:h-11"
                onClick={() => requireSavedPluginState() && setInstallOpen(true)}
              >
                <PackagePlus />
                {t('plugins:pluginsPage.installPlugin')}
              </Button>
            )
          }
        >
          <Segmented
            options={sourceFilters.map((source) => sourceLabel(source, t))}
            value={sourceLabel(sourceFilter, t)}
            onChange={(label) =>
              setSourceFilter(
                sourceFilters.find((source) => sourceLabel(source, t) === label) || 'all',
              )
            }
          />
        </PageToolbar>

        <section className={CARD}>
          <header className={cn('plugin-catalog-heading', CARD_HEAD)}>
            <h2 className="flex min-w-0 items-baseline gap-2 text-sm font-medium text-content">
              {t('plugins:pluginsPage.toolCatalog')}
              <span className="font-normal text-content-muted tabular-nums">{filtered.length}</span>
            </h2>
            <span className="plugin-catalog-summary flex min-w-0 flex-wrap items-center justify-end gap-2 text-xs text-content-secondary tabular-nums">
              {dirty && <Badge tone="amber">{t('plugins:pluginsPage.unsaved')}</Badge>}
              <span>
                <strong className="font-medium text-content">
                  {entries.filter((tool) => tool.enabled).length}
                </strong>
                /{entries.length} {t('plugins:pluginsPage.enabled2')}
              </span>
            </span>
          </header>
          {filtered.length ? (
            <ul className="plugin-tool-grid divide-y divide-stroke-soft">
              {filtered.map((tool) => {
                const open = expandedName === tool.name
                const Icon = TOOL_ICONS[tool.name] || Plug
                return (
                  <li
                    className="plugin-tool-card group/tool min-w-0"
                    data-open={open || undefined}
                    key={tool.name}
                  >
                    <div className="plugin-tool-card-head grid min-h-[52px] grid-cols-[minmax(0,1fr)_auto] items-center gap-3 py-1.5 pr-2 pl-3.5 transition-colors duration-(--d-fast) group-data-open/tool:bg-surface-subtle hover:bg-surface-subtle max-[650px]:pl-3">
                      {/* 宽屏把风险等级与路径范围展开成两列（与详情同源），窄屏只留名称。 */}
                      <button
                        type="button"
                        className="grid min-w-0 grid-cols-[28px_minmax(0,1fr)] items-center gap-3 self-stretch text-left focus-visible:-outline-offset-2 @min-[880px]/page:grid-cols-[28px_minmax(0,1fr)_96px_minmax(0,200px)]"
                        aria-expanded={open}
                        onClick={() =>
                          setExpandedName((current) => (current === tool.name ? '' : tool.name))
                        }
                      >
                        <span className={ICON_KEY}>
                          <Icon />
                        </span>
                        <span className="plugin-tool-title flex min-w-0 flex-col gap-0.5">
                          <strong
                            className="truncate text-sm font-medium text-content"
                            title={displayToolName(tool, t)}
                          >
                            {displayToolName(tool, t)}
                          </strong>
                          <small className="flex min-w-0 items-baseline gap-1.5 text-xs text-content-muted">
                            <code className="min-w-0 truncate font-mono">{tool.name}</code>
                            <span aria-hidden="true" className="text-content-faint">
                              ·
                            </span>
                            <span className="shrink-0">{displayPluginName(tool, t)}</span>
                          </small>
                        </span>
                        {/* 风险等级用 6px 状态点承载颜色，文字保持中性，避免整列告警色。 */}
                        <span className="hidden items-center gap-1.5 text-xs text-content-secondary @min-[880px]/page:flex">
                          <i
                            aria-hidden="true"
                            className={cn(
                              'size-1.5 shrink-0 rounded-full',
                              isHighRisk(tool.risk)
                                ? 'bg-danger'
                                : isMediumRisk(tool.risk)
                                  ? 'bg-warning-strong'
                                  : 'bg-content-faint',
                            )}
                          />
                          {toolRiskLabel(tool.risk, t)}
                        </span>
                        <span className="hidden truncate text-xs text-content-muted @min-[880px]/page:block">
                          {toolScopeLabel({ id: tool.name, scope: tool.scope }, t)}
                        </span>
                      </button>
                      <span className="plugin-tool-trailing inline-flex items-center gap-2">
                        {!tool.pluginBuiltIn && (
                          <Badge tone="amber">{t('plugins:pluginsPage.local')}</Badge>
                        )}
                        <Toggle
                          value={tool.enabled}
                          onChange={(enabled) =>
                            toggleCapability(tool.pluginId, tool.name, enabled)
                          }
                          ariaLabel={displayToolName(tool, t)}
                        />
                        <button
                          type="button"
                          className="plugin-tool-chevron grid size-7 place-items-center rounded-sm text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content max-[650px]:size-11 [&_svg]:transition-transform [&_svg]:duration-(--d-base) [&[aria-expanded=true]_svg]:rotate-180"
                          aria-label={t('plugins:pluginsPage.toggleToolDetails', {
                            name: displayToolName(tool, t),
                          })}
                          aria-expanded={open}
                          onClick={() =>
                            setExpandedName((current) => (current === tool.name ? '' : tool.name))
                          }
                        >
                          <ChevronDown className="size-4" />
                        </button>
                      </span>
                    </div>

                    {open && (
                      <div className="plugin-tool-details grid gap-3 border-t border-stroke-soft bg-surface-subtle py-3 pr-3.5 pl-[54px] max-[650px]:pl-3">
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
                            <dd className={isHighRisk(tool.risk) ? 'text-danger!' : ''}>
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
                          <div className="plugin-tool-warning flex items-start gap-2 rounded-sm bg-warning-soft p-2.5 text-xs leading-normal text-warning-strong">
                            <ShieldAlert className="mt-px size-3.5 shrink-0" />
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
                    )}
                  </li>
                )
              })}
            </ul>
          ) : (
            <InlineEmptyState compact title={t('plugins:pluginsPage.noMatchingTools')} />
          )}
        </section>

        <ExtensionMarket
          query={query}
          notify={notify}
          requestText={requestText}
          requestConfirm={requestConfirm}
        />

        {customPluginsAvailable && (
          <PluginInstallDialog
            open={installOpen}
            onOpenChange={setInstallOpen}
            onInstalled={load}
          />
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

        {error && (
          <AppError className="fixed right-6 bottom-6 z-[21] max-w-[360px] border border-danger-border shadow-float">
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}
      </div>
    </PageFrame>
  )
}
