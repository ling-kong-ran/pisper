import './provider-messages'
// Provider 导航、连接与模型列表共用同一详情页；视觉模型不再要求独立连接。
import { useState } from 'react'
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
import { SettingsSwitch } from './settings-primitives'
import type { ConfigData, ProviderConfig, ProviderModel } from './config-types'
type Props = {
  config: ConfigData
  selectedProviderId: string
  onSelectProvider: (id: string) => void
  toggling: string
  settingDefault: string
  settingModel: string
  onSave: (config: ConfigData) => void
  onAdd: () => void
  onClone: (provider: ProviderConfig) => void
  onDelete: (provider: ProviderConfig) => void | Promise<void>
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onSetDefault: (provider: ProviderConfig) => void | Promise<void>
  onSetDefaultModel: (provider: ProviderConfig, model: string) => void | Promise<void>
}

export function ProviderWorkbench(props: Props) {
  const { t } = useI18n()
  const defaultId = props.config.defaultProvider || props.config.provider
  const providers = props.config.providers
  const selectedId = props.selectedProviderId || defaultId
  // 草稿仅在本次页面驻留期间存活，切换 Provider 不丢编辑，也不落盘密钥。
  const [drafts, setDrafts] = useState<Record<string, ProviderConnectionDraft | undefined>>({})
  const [connectionSaving, setConnectionSaving] = useState(false)
  const [editing, setEditing] = useState<{
    provider: ProviderConfig
    model?: ProviderModel
  } | null>(null)
  const selected =
    providers.find((provider) => provider.id === selectedId) ??
    providers.find((provider) => provider.id === defaultId) ??
    providers[0]
  const Icon = selected ? PROVIDER_ICONS[selected.id] || Server : Server
  const busy = Boolean(
    connectionSaving || props.settingDefault || props.settingModel || props.toggling,
  )
  return (
    <section
      data-config-card="models-connections"
      className="overflow-hidden rounded-xl border border-border bg-card"
    >
      <div
        data-model-provider-split-panel
        className="grid min-h-[36rem] grid-cols-[56px_minmax(0,1fr)] md:grid-cols-[224px_minmax(0,1fr)]"
      >
        <nav
          aria-label={t('config:configPage.connections')}
          className="min-w-0 border-r border-border p-2 md:p-3"
        >
          <div className="mb-3 flex h-8 items-center justify-between px-1 text-xs text-muted-foreground">
            <span className="max-md:sr-only">{t('config:configPage.connections')}</span>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t('config:configPage.addCustomConnection')}
              title={t('config:configPage.addCustomConnection')}
              disabled={busy}
              onClick={props.onAdd}
            >
              <Plus size={16} />
            </Button>
          </div>
          {[false, true].map((custom) => (
            <div key={String(custom)} className="mb-4 space-y-1">
              <p className="px-2 py-1.5 text-xs text-muted-foreground max-md:sr-only">
                {custom
                  ? t('config:providerWorkbench.custom')
                  : t('config:providerWorkbench.presets')}
              </p>
              {providers
                .filter((provider) => Boolean(provider.custom) === custom)
                .map((provider) => {
                  const ProviderIcon = PROVIDER_ICONS[provider.id] || Server
                  return (
                    <button
                      key={provider.id}
                      type="button"
                      title={provider.name}
                      aria-label={provider.name}
                      aria-current={selected?.id === provider.id ? 'true' : undefined}
                      disabled={busy}
                      onClick={() => props.onSelectProvider(provider.id)}
                      className={cn(
                        'flex min-h-10 w-full items-center gap-2.5 rounded-lg border px-2 text-left text-sm max-md:justify-center max-md:px-0',
                        selected?.id === provider.id
                          ? 'border-border bg-muted text-foreground'
                          : 'border-transparent text-muted-foreground hover:bg-muted/60 hover:text-foreground',
                      )}
                    >
                      <ProviderIcon size={17} className="shrink-0" />
                      <span className="min-w-0 flex-1 truncate max-md:sr-only">
                        {provider.name}
                      </span>
                      <span
                        className={cn(
                          'size-1.5 shrink-0 rounded-full max-md:hidden',
                          provider.configured && provider.enabled
                            ? 'bg-emerald-500'
                            : 'bg-muted-foreground/25',
                        )}
                      />
                    </button>
                  )
                })}
            </div>
          ))}
        </nav>
        <div className="min-w-0 p-4 sm:p-6">
          {selected ? (
            <>
              <div className="mb-6 flex min-w-0 items-center gap-3">
                <span className="grid size-10 shrink-0 place-items-center rounded-xl border border-border">
                  <Icon size={23} />
                </span>
                <div className="min-w-0 flex-1">
                  <h2 className="truncate text-lg font-semibold">{selected.name}</h2>
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
                          onClick={() => setEditing({ provider: selected, model })}
                        >
                          <SlidersHorizontal size={14} />
                        </Button>
                      </div>
                    )
                  })}
                </div>
              </div>
            </>
          ) : (
            <div className="grid min-h-80 place-content-center gap-4 text-center">
              <p className="text-sm">{t('config:configPage.noModelConfiguredYet')}</p>
              <Button onClick={props.onAdd}>
                <Plus size={14} />
                {t('config:configPage.addCustomConnection')}
              </Button>
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
