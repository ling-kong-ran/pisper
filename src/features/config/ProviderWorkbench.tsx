import './provider-messages'
// Provider 导航、连接与模型列表共用同一详情页；视觉模型不再要求独立连接。
// 布局：左侧连接列表卡（字母标记键帽 + 状态点，选中行带 3px 强调刻线），
// 右侧详情卡（标题区 → 标签在左的连接表单 → 模型细线表 → 底部保存栏）。
import { useId, useState } from 'react'
import {
  Brain,
  Copy,
  Image,
  MessageSquare,
  MoreHorizontal,
  Plus,
  SlidersHorizontal,
  Star,
  Trash2,
  Video,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { StarOrbit } from '@/components/StarOrbit'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useUnsavedChanges } from '@/hooks/useUnsavedChanges'
import { cn } from '@/lib/utils'
import { providerApiOptions } from './provider-constants'
import {
  ProviderConnectionEditor,
  isProviderConnectionDirty,
  type ProviderConnectionDraft,
} from './ProviderConnectionEditor'
import { ProviderModelEditor } from './ProviderModelEditor'
import { getProviderWorkbenchState } from './provider-workbench-state'
import { SettingsSwitch } from './settings-primitives'
import type { ConfigData, ProviderConfig, ProviderModel } from './config-types'
type Props = {
  config: ConfigData
  selectedProviderId: string
  onSelectProvider: (id: string) => void
  toggling: string
  settingDefault: string
  settingModel: string
  deletingModel: string
  onSave: (config: ConfigData) => void
  onClone: (provider: ProviderConfig) => void
  onDelete: (provider: ProviderConfig) => void | Promise<void>
  onDeleteModel: (provider: ProviderConfig, model: ProviderModel) => void | Promise<void>
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onSetDefault: (provider: ProviderConfig) => void | Promise<void>
  onSetDefaultModel: (provider: ProviderConfig, model: string) => void | Promise<void>
  // 列表底部的「添加连接」入口；未提供时（如独立夹具）不渲染。
  onAddConnection?: () => void
}

// 字母标记：多词名称取前两个词首字母（OpenAI Codex → OA），单词取前两个字符（Anthropic → An）。
// 用 Array.from 按码点切分，避免把代理对或 CJK 字符截成半个。
function providerMark(name: string) {
  const words = name
    .trim()
    .split(/[\s_\-/.]+/)
    .filter(Boolean)
  if (words.length > 1) return (Array.from(words[0])[0] + Array.from(words[1])[0]).toUpperCase()
  const chars = Array.from(words[0] || '?')
  return chars[0].toUpperCase() + (chars[1] || '')
}

function ProviderMark({ name, large = false }: { name: string; large?: boolean }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'grid shrink-0 place-items-center border border-key-border bg-key font-semibold text-content shadow-key',
        large ? 'size-9.5 rounded-lg text-sm' : 'size-7.5 rounded-md text-xs',
      )}
    >
      {providerMark(name)}
    </span>
  )
}

// 上下文窗口用紧凑读数（1.05M / 200K），配合等宽数字右对齐成列。
function formatContextWindow(tokens?: number) {
  if (!tokens) return '—'
  if (tokens >= 1_000_000) return `${Number((tokens / 1_000_000).toFixed(2))}M`
  if (tokens >= 1000) return `${Math.round(tokens / 1000)}K`
  return String(tokens)
}

function providerApiLabel(api: string) {
  return providerApiOptions(api).find(([value]) => value === api)?.[1] || api
}

// 模型行列宽：ID / 名称 / 上下文 / 能力 / 操作；窄卡里只保留 ID 列与操作列，其余读数折到 ID 下方。
const MODEL_GRID =
  'grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 px-4 @min-[560px]/card:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_64px_76px_minmax(88px,auto)]'

export function ProviderWorkbench(props: Props) {
  const { t } = useI18n()
  const defaultBadgeId = useId()
  const { providers, selected, defaultId } = getProviderWorkbenchState(
    props.config,
    props.selectedProviderId,
  )
  // 草稿仅在本次页面驻留期间存活，切换 Provider 不丢编辑，也不落盘密钥。
  const [drafts, setDrafts] = useState<Record<string, ProviderConnectionDraft | undefined>>({})
  const [connectionSaving, setConnectionSaving] = useState(false)
  const [editing, setEditing] = useState<{
    provider: ProviderConfig
    model?: ProviderModel
  } | null>(null)
  const hasProviders = providers.length > 0
  const busy = Boolean(
    connectionSaving ||
    props.settingDefault ||
    props.settingModel ||
    props.toggling ||
    props.deletingModel,
  )
  useUnsavedChanges({
    id: `provider-drafts-${defaultBadgeId}`,
    dirty: providers.some((provider) =>
      Boolean(drafts[provider.id] && isProviderConnectionDirty(provider, drafts[provider.id]!)),
    ),
    saving: busy,
    discard: () => setDrafts({}),
  })
  const healthy = Boolean(selected?.configured && selected.enabled)
  const statusText = !selected
    ? ''
    : !selected.configured
      ? selected.id === 'openai-codex'
        ? t('config:configPage.codexCLILoginRequired')
        : t('config:configPage.apiKeyRequired')
      : selected.enabled
        ? t('config:configPage.authenticationReady')
        : t('config:configPage.disabled2')
  const capabilityLabels = {
    chat: t('providers:modelEditor.chat'),
    image: t('providers:modelEditor.image'),
    video: t('providers:modelEditor.video'),
    reasoning: t('providers:modelEditor.reasoning'),
  }
  const capabilityIcons = (model: ProviderModel) => {
    const capabilities = model.capabilities || [model.kind]
    const items = (
      [
        ['chat', MessageSquare, capabilities.includes('chat')],
        ['image', Image, capabilities.includes('image')],
        ['video', Video, capabilities.includes('video')],
        ['reasoning', Brain, Boolean(model.reasoning) && capabilities.includes('chat')],
      ] as const
    ).filter(([, , shown]) => shown)
    return items.map(([key, Icon]) => (
      <span key={key} title={capabilityLabels[key]} className="inline-flex">
        <Icon size={13} aria-hidden="true" />
        <span className="sr-only">{capabilityLabels[key]}</span>
      </span>
    ))
  }
  return (
    <section data-config-card="models-connections" className="min-w-0">
      <div
        data-model-provider-split-panel
        className={cn(
          'grid min-w-0 grid-cols-1 content-start items-start gap-5',
          hasProviders && '@min-[840px]/page:grid-cols-[248px_minmax(0,1fr)]',
        )}
      >
        {hasProviders && (
          <div className="min-w-0 overflow-hidden rounded-md border border-stroke bg-panel">
            <p className="flex h-9 items-center gap-1.5 border-b border-stroke-soft bg-surface-subtle px-3 text-xs font-medium text-content-muted">
              {t('config:configPage.connections')}
              <span className="tabular-nums">{providers.length}</span>
            </p>
            <nav aria-label={t('config:configPage.connections')} className="min-w-0">
              <div className="divide-y divide-stroke-soft">
                {providers.map((provider) => {
                  const isDefault = provider.id === defaultId
                  const isSelected = selected?.id === provider.id
                  return (
                    <button
                      key={provider.id}
                      type="button"
                      title={provider.name}
                      aria-label={provider.name}
                      aria-describedby={isDefault ? defaultBadgeId : undefined}
                      aria-current={isSelected ? 'true' : undefined}
                      disabled={busy}
                      onClick={() => props.onSelectProvider(provider.id)}
                      className={cn(
                        'relative flex min-h-14 w-full min-w-0 items-center gap-2.5 px-3 py-2.5 text-left transition-colors max-[650px]:min-h-16',
                        isSelected
                          ? 'bg-surface-muted before:absolute before:inset-y-2.5 before:left-0 before:w-[3px] before:rounded-r-[3px] before:bg-star'
                          : 'hover:bg-surface-subtle',
                      )}
                    >
                      <ProviderMark name={provider.name} />
                      <span className="min-w-0 flex-1">
                        <span className="flex min-w-0 items-center gap-1.5">
                          <span className="truncate text-sm font-medium text-content">
                            {provider.name}
                          </span>
                          {isDefault && (
                            <Badge id={defaultBadgeId} variant="accent">
                              {t('config:providerWorkbench.default')}
                            </Badge>
                          )}
                        </span>
                        <span className="mt-0.5 block truncate text-xs text-content-muted">
                          <span className="tabular-nums">
                            {t('config:configPage.countModels', {
                              count: provider.models.length,
                            })}
                          </span>
                          {' · '}
                          {providerApiLabel(provider.api)}
                        </span>
                      </span>
                      <span
                        aria-hidden="true"
                        className={cn(
                          'size-1.5 shrink-0 rounded-full',
                          provider.configured && provider.enabled
                            ? 'bg-success'
                            : 'bg-content-faint',
                        )}
                      />
                    </button>
                  )
                })}
              </div>
            </nav>
            {props.onAddConnection && (
              <button
                type="button"
                disabled={busy}
                onClick={props.onAddConnection}
                className="flex h-10 w-full items-center gap-2 border-t border-stroke-soft px-3 text-sm text-content-secondary transition-colors hover:bg-surface-subtle hover:text-content max-[650px]:h-11"
              >
                <Plus size={16} aria-hidden="true" className="text-content-muted" />
                {t('config:providerWorkbench.addConnection')}
              </button>
            )}
          </div>
        )}
        {selected ? (
          <div className="@container/card min-w-0 overflow-clip rounded-md border border-stroke bg-panel">
            <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2 border-b border-stroke-soft px-4 py-3.5">
              <ProviderMark name={selected.name} large />
              <div className="min-w-0 flex-1 basis-40">
                <div className="flex min-w-0 items-center gap-2">
                  <h2 className="min-w-0 truncate text-[15px] leading-6 font-medium text-content">
                    {selected.name}
                  </h2>
                  {selected.id === defaultId && (
                    <Badge variant="accent">{t('config:providerWorkbench.default')}</Badge>
                  )}
                </div>
                <p className="truncate font-mono text-xs text-content-muted">{selected.id}</p>
              </div>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <span className="inline-flex items-center gap-1.5 text-xs whitespace-nowrap text-content-muted">
                  <span
                    aria-hidden="true"
                    className={cn(
                      'size-1.5 rounded-full',
                      healthy ? 'bg-success' : 'bg-content-faint',
                    )}
                  />
                  <span className={cn(healthy && 'font-medium text-success')}>{statusText}</span>
                </span>
                <span className="flex items-center gap-2 border-l border-stroke pl-3 text-sm text-content-secondary">
                  <span aria-hidden="true">{t('config:configPage.enabled')}</span>
                  <SettingsSwitch
                    value={selected.configured && selected.enabled}
                    disabled={!selected.configured || busy}
                    onChange={() => void props.onToggle(selected, !selected.enabled)}
                    ariaLabel={t('config:configPage.providerEnabled', { name: selected.name })}
                  />
                </span>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      className="pointer-coarse:size-11 max-[650px]:size-11"
                      aria-label={t('config:providerWorkbench.actions')}
                    >
                      <MoreHorizontal size={16} />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem
                      disabled={
                        selected.id === defaultId ||
                        !selected.configured ||
                        !selected.enabled ||
                        !selected.defaultModel ||
                        busy
                      }
                      onSelect={() => void props.onSetDefault(selected)}
                    >
                      <Star size={14} />
                      {t('config:configPage.setAsDefaultProvider')}
                    </DropdownMenuItem>
                    <DropdownMenuItem disabled={busy} onSelect={() => props.onClone(selected)}>
                      <Copy size={14} />
                      {t('config:configPage.cloneProvider')}
                    </DropdownMenuItem>
                    {selected.custom && (
                      <DropdownMenuItem
                        className="text-destructive"
                        disabled={busy}
                        onSelect={() => void props.onDelete(selected)}
                      >
                        <Trash2 size={14} />
                        {t('config:configPage.deleteProvider')}
                      </DropdownMenuItem>
                    )}
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </div>
            <ProviderConnectionEditor
              key={selected.id}
              provider={selected}
              draft={
                drafts[selected.id] ?? {
                  name: selected.name,
                  api: selected.api,
                  baseUrl: selected.baseUrl || '',
                  apiKey: '',
                }
              }
              onDraftChange={(draft) =>
                setDrafts((current) => ({ ...current, [selected.id]: draft }))
              }
              onSavingChange={setConnectionSaving}
              onSave={props.onSave}
            >
              <div className="flex h-11.5 items-center gap-2 border-t border-stroke bg-surface-subtle pr-3 pl-4">
                <h3 className="text-sm font-medium text-content">
                  {t('config:providerWorkbench.models')}
                </h3>
                <span className="text-xs text-content-muted tabular-nums">
                  {selected.models.length}
                </span>
                <Button
                  variant="secondary"
                  size="sm"
                  className="ml-auto pointer-coarse:h-11 max-[650px]:h-11"
                  disabled={busy}
                  onClick={() => setEditing({ provider: selected })}
                >
                  <Plus size={14} />
                  {t('providers:modelEditor.add')}
                </Button>
              </div>
              {selected.models.length > 0 && (
                <div
                  className={cn(
                    MODEL_GRID,
                    'hidden h-8 border-t border-stroke-soft text-xs font-medium text-content-muted @min-[560px]/card:grid',
                  )}
                >
                  <span>{t('providers:modelEditor.id')}</span>
                  <span>{t('config:configPage.displayName')}</span>
                  <span className="text-right">{t('config:providerWorkbench.contextColumn')}</span>
                  <span>{t('providers:modelEditor.capabilities')}</span>
                  <span aria-hidden="true" />
                </div>
              )}
              {selected.models.map((model) => {
                const isDefaultModel = selected.defaultModel === model.id
                return (
                  <div
                    key={model.id}
                    data-provider-model={model.id}
                    className={cn(
                      MODEL_GRID,
                      'min-h-11.5 border-t border-stroke-soft py-px text-sm text-content-secondary transition-colors hover:bg-surface-subtle',
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => setEditing({ provider: selected, model })}
                      disabled={busy}
                      className="flex min-h-11 min-w-0 flex-col justify-center rounded-xs py-1 text-left"
                      aria-label={t('providers:modelEditor.editNamed', {
                        name: model.name || model.id,
                      })}
                    >
                      <span className="block truncate font-mono text-[12.5px] text-content">
                        {model.id}
                      </span>
                      {/* 窄卡：名称、上下文与能力折叠为 ID 下的一行读数。 */}
                      <span className="mt-0.5 flex min-w-0 items-center gap-2 text-xs text-content-muted @min-[560px]/card:hidden">
                        <span className="min-w-0 truncate">{model.name || model.id}</span>
                        <span className="tabular-nums">
                          {formatContextWindow(model.contextWindow)}
                        </span>
                        {capabilityIcons(model)}
                      </span>
                    </button>
                    <span className="hidden min-w-0 truncate text-content @min-[560px]/card:block">
                      {model.name || model.id}
                    </span>
                    <span className="hidden text-right tabular-nums @min-[560px]/card:block">
                      {formatContextWindow(model.contextWindow)}
                    </span>
                    <span className="hidden items-center gap-2 text-content-muted @min-[560px]/card:flex">
                      {capabilityIcons(model)}
                    </span>
                    <div className="flex items-center justify-end gap-0.5">
                      {model.kind === 'chat' && (
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className={cn(
                            'pointer-coarse:size-11 max-[650px]:size-11',
                            // 默认模型的实心星是状态读数而非禁用控件，保持满不透明度。
                            isDefaultModel &&
                              'text-star disabled:opacity-100 aria-disabled:opacity-100',
                          )}
                          aria-label={t('config:configPage.providerDefaultModel') + ': ' + model.id}
                          title={t('config:configPage.providerDefaultModel')}
                          aria-pressed={isDefaultModel}
                          disabled={busy || !selected.configured || isDefaultModel}
                          onClick={() => void props.onSetDefaultModel(selected, model.id)}
                        >
                          <Star size={15} className={cn(isDefaultModel && 'fill-current')} />
                        </Button>
                      )}
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="pointer-coarse:size-11 max-[650px]:size-11"
                        aria-label={t('providers:modelEditor.optionsNamed', { name: model.id })}
                        disabled={busy}
                        onClick={() => setEditing({ provider: selected, model })}
                      >
                        <SlidersHorizontal size={14} />
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        className="hover:bg-danger-soft hover:text-danger pointer-coarse:size-11 max-[650px]:size-11"
                        disabled={busy}
                        aria-label={t('providers:modelEditor.deleteNamed', {
                          name: model.name || model.id,
                        })}
                        title={t('providers:modelEditor.delete')}
                        onClick={() => void props.onDeleteModel(selected, model)}
                      >
                        <Trash2 size={14} />
                      </Button>
                    </div>
                  </div>
                )
              })}
            </ProviderConnectionEditor>
          </div>
        ) : (
          <div className="grid min-h-80 place-content-center justify-items-center gap-2 rounded-lg border border-dashed border-stroke-strong px-6 text-center">
            <StarOrbit size={48} className="mb-2" />
            <p className="text-sm font-medium text-content">
              {t('config:providerWorkbench.noConnections')}
            </p>
            <p className="max-w-sm text-xs text-content-muted">
              {t('config:providerWorkbench.addConnectionHint')}
            </p>
          </div>
        )}
      </div>
      {editing && (
        <ProviderModelEditor
          provider={editing.provider}
          model={editing.model}
          onSave={props.onSave}
          onClose={() => setEditing(null)}
        />
      )}
    </section>
  )
}
