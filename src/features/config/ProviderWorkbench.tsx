import './provider-messages'
// Provider 导航、连接与模型列表共用同一详情页；视觉模型不再要求独立连接。
import { useId, useState } from 'react'
import {
  Check,
  Copy,
  MoreHorizontal,
  Plus,
  Server,
  Star,
  Trash2,
  SlidersHorizontal,
  Brain,
  Image,
  MessageSquare,
  Video,
} from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import { Badge } from '@/components/ui/badge'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { cn } from '@/lib/utils'
import { PROVIDER_ICONS } from './provider-constants'
import { ProviderConnectionEditor, type ProviderConnectionDraft } from './ProviderConnectionEditor'
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
}

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
  const Icon = selected ? PROVIDER_ICONS[selected.id] || Server : Server
  const hasProviders = providers.length > 0
  const busy = Boolean(
    connectionSaving ||
    props.settingDefault ||
    props.settingModel ||
    props.toggling ||
    props.deletingModel,
  )
  return (
    <section
      data-config-card="models-connections"
      className="overflow-hidden rounded-xl border border-border bg-card"
    >
      <div
        data-model-provider-split-panel
        className={cn(
          'grid grid-cols-1 content-start',
          hasProviders && 'min-h-[36rem] md:grid-cols-[224px_minmax(0,1fr)]',
        )}
      >
        {hasProviders && (
          <nav
            aria-label={t('config:configPage.connections')}
            className="min-w-0 border-b border-border p-3 md:border-r md:border-b-0"
          >
            <p className="mb-2 px-2 text-xs text-muted-foreground">
              {t('config:configPage.connections')}
            </p>
            <div className="flex gap-1 overflow-x-auto pb-1 md:flex-col md:overflow-x-visible md:pb-0">
              {providers.map((provider) => {
                const ProviderIcon = PROVIDER_ICONS[provider.id] || Server
                const isDefault = provider.id === defaultId
                return (
                  <button
                    key={provider.id}
                    type="button"
                    title={provider.name}
                    aria-label={provider.name}
                    aria-describedby={isDefault ? defaultBadgeId : undefined}
                    aria-current={selected?.id === provider.id ? 'true' : undefined}
                    disabled={busy}
                    onClick={() => props.onSelectProvider(provider.id)}
                    className={cn(
                      'flex min-h-11 max-w-64 shrink-0 items-center gap-2.5 rounded-lg border px-3 text-left text-sm md:min-h-10 md:w-full md:px-2',
                      selected?.id === provider.id
                        ? 'border-border bg-muted text-foreground'
                        : 'border-transparent text-muted-foreground hover:bg-muted/60 hover:text-foreground',
                    )}
                  >
                    <ProviderIcon size={17} className="shrink-0" />
                    <span className="min-w-0 flex-1 truncate">{provider.name}</span>
                    {isDefault && (
                      <Badge id={defaultBadgeId} variant="secondary" className="px-1.5">
                        {t('config:providerWorkbench.default')}
                      </Badge>
                    )}
                    <span
                      className={cn(
                        'size-1.5 shrink-0 rounded-full',
                        provider.configured && provider.enabled
                          ? 'bg-emerald-500'
                          : 'bg-muted-foreground/25',
                      )}
                    />
                  </button>
                )
              })}
            </div>
          </nav>
        )}
        <div className="min-w-0 p-4 sm:p-6">
          {selected ? (
            <>
              <div className="mb-6 flex min-w-0 items-center gap-3">
                <span className="grid size-10 shrink-0 place-items-center rounded-xl border border-border">
                  <Icon size={23} />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
                    <h2 className="max-w-full truncate text-lg font-semibold">{selected.name}</h2>
                    {selected.id === defaultId && (
                      <Badge variant="secondary">
                        {t('config:providerWorkbench.defaultProvider')}
                      </Badge>
                    )}
                  </div>
                  <p className="truncate text-xs text-muted-foreground">{selected.id}</p>
                </div>
                <SettingsSwitch
                  value={selected.configured && selected.enabled}
                  disabled={!selected.configured || busy}
                  onChange={() => void props.onToggle(selected, !selected.enabled)}
                  ariaLabel={t('config:configPage.providerEnabled', { name: selected.name })}
                />
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button
                      variant="ghost"
                      size="icon-sm"
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
                onDraftChange={(draft) =>
                  setDrafts((current) => ({ ...current, [selected.id]: draft }))
                }
                onSavingChange={setConnectionSaving}
                onSave={props.onSave}
              />
              <div className="mt-6 border-t border-border pt-5">
                <div className="mb-3 flex items-center justify-between gap-2">
                  <h3 className="text-sm font-medium">
                    {t('config:providerWorkbench.models')}{' '}
                    <span className="ml-1 font-normal text-muted-foreground">
                      {selected.models.length}
                    </span>
                  </h3>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busy}
                    onClick={() => setEditing({ provider: selected })}
                  >
                    <Plus size={14} />
                    {t('providers:modelEditor.add')}
                  </Button>
                </div>
                <div className="space-y-1.5">
                  {selected.models.map((model) => {
                    const capabilities = model.capabilities || [model.kind]
                    return (
                      <div
                        key={model.id}
                        data-provider-model={model.id}
                        className="flex min-w-0 items-center gap-2 rounded-lg border border-border/70 px-3 py-2.5"
                      >
                        <button
                          type="button"
                          onClick={() => setEditing({ provider: selected, model })}
                          disabled={busy}
                          className="min-w-0 flex-1 text-left"
                          aria-label={t('providers:modelEditor.editNamed', {
                            name: model.name || model.id,
                          })}
                        >
                          <span className="block truncate text-sm font-medium">
                            {model.name || model.id}
                          </span>
                          <span className="mt-0.5 flex items-center gap-2 text-xs text-muted-foreground">
                            <span className="min-w-0 truncate">{model.id}</span>
                            {capabilities.includes('chat') && <MessageSquare size={12} />}{' '}
                            {capabilities.includes('image') && <Image size={12} />}{' '}
                            {capabilities.includes('video') && <Video size={12} />}{' '}
                            {model.reasoning && capabilities.includes('chat') && (
                              <Brain size={12} />
                            )}
                          </span>
                        </button>
                        {model.kind === 'chat' && (
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            aria-label={
                              t('config:configPage.providerDefaultModel') + ': ' + model.id
                            }
                            title={t('config:configPage.providerDefaultModel')}
                            aria-pressed={selected.defaultModel === model.id}
                            disabled={
                              busy || !selected.configured || selected.defaultModel === model.id
                            }
                            onClick={() => void props.onSetDefaultModel(selected, model.id)}
                          >
                            {selected.defaultModel === model.id ? (
                              <Check size={15} />
                            ) : (
                              <Star size={15} />
                            )}
                          </Button>
                        )}
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={t('providers:modelEditor.optionsNamed', { name: model.id })}
                          disabled={busy}
                          onClick={() => setEditing({ provider: selected, model })}
                        >
                          <SlidersHorizontal size={14} />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon-sm"
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
                    )
                  })}
                </div>
              </div>
            </>
          ) : (
            <div className="grid min-h-80 place-content-center gap-4 text-center">
              <Server size={28} className="mx-auto text-muted-foreground" />
              <p className="text-sm font-medium">{t('config:providerWorkbench.noConnections')}</p>
              <p className="max-w-sm text-sm text-muted-foreground">
                {t('config:providerWorkbench.addConnectionHint')}
              </p>
            </div>
          )}
        </div>
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
