import './provider-messages'
// 设置 › 模型的“提供方”分区（方向 A，效果图 desktop-settings-light.png、样稿 settings.html）：
// 每个提供方一行（字母标记 · 名称 · 密钥与模型数 · 右侧“已连接 / 未连接”），与功能页同一个 FeatureRow；
// 展开一行在原位编辑：启用开关与操作菜单 → 标签在左的连接表单 → 模型细线表 → 保存。同一时刻只展开一行。
// 原先左“连接”导航卡 + 右详情卡、灰底分区头、卡片套卡片的两栏工作台随之删除。视觉模型不再要求独立连接。
import { useId, useState, type ReactNode } from 'react'
import {
  Brain,
  Check,
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
import { FeatureList, FeatureRow, FeatureSection } from '@/components/app/FeaturePage'
import { StarOrbit } from '@/components/StarOrbit'
import { Button } from '@/components/ui/button'
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
  // 列表底部的「添加提供方」入口；未提供时（如独立夹具）不渲染。
  onAddConnection?: () => void
  // 分区标题右侧的读数（本地配置自动接入的计数）。
  meta?: ReactNode
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

// 模型行列宽：ID / 名称 / 上下文 / 能力 / 操作；详情窄时（手机）只保留 ID 列与操作列，其余读数折到 ID 下方。
const MODEL_GRID =
  'grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 @min-[560px]/card:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_64px_76px_minmax(88px,auto)]'

export function ProviderWorkbench(props: Props) {
  const { t } = useI18n()
  const draftsId = useId()
  const { providers, defaultId } = getProviderWorkbenchState(props.config, props.selectedProviderId)
  // 展开的行就是正在编辑的提供方；'' 表示全部收起（进入页面时的默认）。
  const expanded = providers.find((provider) => provider.id === props.selectedProviderId)
  // 草稿仅在本次页面驻留期间存活，收起或换展开另一行都不丢编辑，也不落盘密钥。
  const [drafts, setDrafts] = useState<Record<string, ProviderConnectionDraft | undefined>>({})
  const [connectionSaving, setConnectionSaving] = useState(false)
  const [editing, setEditing] = useState<{
    provider: ProviderConfig
    model?: ProviderModel
  } | null>(null)
  const busy = Boolean(
    connectionSaving ||
    props.settingDefault ||
    props.settingModel ||
    props.toggling ||
    props.deletingModel,
  )
  useUnsavedChanges({
    id: `provider-drafts-${draftsId}`,
    dirty: providers.some((provider) =>
      Boolean(drafts[provider.id] && isProviderConnectionDirty(provider, drafts[provider.id]!)),
    ),
    saving: busy,
    discard: () => setDrafts({}),
  })
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
  // 行里第二行的读数：密钥状态 · 模型数 · 协议。
  const keyState = (provider: ProviderConfig) =>
    provider.configured
      ? t('config:providerWorkbench.keyConfigured')
      : provider.id === 'openai-codex'
        ? t('config:configPage.codexCLILoginRequired')
        : t('config:configPage.apiKeyRequired')
  // 右侧状态一律中性（规格 6.1）：已连接带 ✓，已停用与未连接只是文字。
  const connectionState = (provider: ProviderConfig) =>
    !provider.configured ? (
      t('config:providerWorkbench.notConnected')
    ) : provider.enabled ? (
      <span className="inline-flex items-center gap-1 text-content-secondary">
        <Check className="size-3.5" aria-hidden="true" />
        {t('config:providerWorkbench.connected')}
      </span>
    ) : (
      t('config:configPage.disabled2')
    )

  const detail = (selected: ProviderConfig) => (
    <div className="@container/card min-w-0" data-provider-detail={selected.id}>
      {/* 详情第一行：等宽 id、启用开关与操作菜单（设为默认、复制、删除）。 */}
      <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-t border-stroke-soft py-2">
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-content-muted">
          {selected.id}
        </span>
        <span className="flex items-center gap-2 text-sm text-content-secondary">
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
        onDraftChange={(draft) => setDrafts((current) => ({ ...current, [selected.id]: draft }))}
        onSavingChange={setConnectionSaving}
        onSave={props.onSave}
      >
        <div className="mt-2 flex min-h-10 items-center gap-2 border-t border-stroke-soft pt-2">
          <h3 className="text-[13px] font-medium text-content-muted">
            {t('config:providerWorkbench.models')}
            <span className="tabular-nums"> · {selected.models.length}</span>
          </h3>
          <Button
            type="button"
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
              'hidden h-8 text-xs font-medium text-content-muted @min-[560px]/card:grid',
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
                'min-h-11.5 border-t border-stroke-soft py-px text-sm text-content-secondary',
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
                {/* 窄详情：名称、上下文与能力折叠为 ID 下的一行读数。 */}
                <span className="mt-0.5 flex min-w-0 items-center gap-2 text-xs text-content-muted @min-[560px]/card:hidden">
                  <span className="min-w-0 truncate">{model.name || model.id}</span>
                  <span className="tabular-nums">{formatContextWindow(model.contextWindow)}</span>
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
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    className={cn(
                      'pointer-coarse:size-11 max-[650px]:size-11',
                      // 默认模型的实心星是状态读数而非禁用控件，保持满不透明度。
                      isDefaultModel && 'text-star disabled:opacity-100 aria-disabled:opacity-100',
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
                  type="button"
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
                  type="button"
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
  )

  return (
    <FeatureSection
      data-config-card="models-connections"
      data-model-provider-list
      title={t('config:providerWorkbench.providers')}
      count={providers.length}
      meta={props.meta}
      description={t('config:providerWorkbench.providersHint')}
    >
      {providers.length > 0 ? (
        <FeatureList>
          {providers.map((provider) => {
            const isDefault = provider.id === defaultId
            const open = expanded?.id === provider.id
            return (
              <FeatureRow
                key={provider.id}
                data-provider-row={provider.id}
                icon={
                  <span className="text-[13px] font-semibold text-content-secondary">
                    {providerMark(provider.name)}
                  </span>
                }
                name={provider.name}
                description={[
                  keyState(provider),
                  t('config:configPage.countModels', { count: provider.models.length }),
                  providerApiLabel(provider.api),
                ].join(' · ')}
                status={
                  <>
                    {isDefault && (
                      <span data-provider-default className="text-content-secondary">
                        {t('config:providerWorkbench.default')} ·
                      </span>
                    )}
                    {connectionState(provider)}
                  </>
                }
                expanded={open}
                onExpandedChange={(next) => props.onSelectProvider(next ? provider.id : '')}
                toggleLabel={t('config:providerWorkbench.toggleProvider', { name: provider.name })}
                // 收起时传 null（不是 undefined）：FeatureRow 据此认出这是可展开的行。
                detail={open ? detail(provider) : null}
              />
            )
          })}
        </FeatureList>
      ) : (
        <div className="grid min-h-48 place-content-center justify-items-center gap-2 border-y border-stroke px-6 py-8 text-center">
          <StarOrbit size={40} className="mb-1" />
          <p className="text-sm font-medium text-content">
            {t('config:providerWorkbench.noConnections')}
          </p>
          <p className="max-w-sm text-xs text-content-muted">
            {t('config:providerWorkbench.addConnectionHint')}
          </p>
        </div>
      )}
      {props.onAddConnection && (
        <button
          type="button"
          data-add-provider
          disabled={busy}
          onClick={props.onAddConnection}
          className="flex min-h-[52px] w-full min-w-0 items-center gap-2.5 text-left text-sm text-content-secondary transition-colors duration-(--d-fast) hover:text-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus disabled:opacity-50"
        >
          <Plus size={16} aria-hidden="true" className="flex-none text-content-muted" />
          <span className="flex-none">{t('config:providerWorkbench.addProvider')}</span>
          <span className="min-w-0 truncate text-xs text-content-muted">
            {t('config:providerWorkbench.addProviderHint')}
          </span>
        </button>
      )}
      {editing && (
        <ProviderModelEditor
          provider={editing.provider}
          model={editing.model}
          onSave={props.onSave}
          onClose={() => setEditing(null)}
        />
      )}
    </FeatureSection>
  )
}
