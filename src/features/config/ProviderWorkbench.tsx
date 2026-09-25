// ZCode-style provider navigation/detail split, backed by Pisper's configuration API.
import { useState } from 'react'
import { Check, Copy, MoreHorizontal, Plus, Server, Star, Trash2, Wand2 } from 'lucide-react'
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
import { ProviderConfigModal } from './ProviderDialogs'
import { SettingsSwitch } from './settings-primitives'
import type { ConfigData, ProviderConfig } from './config-types'

type Props = {
  config: ConfigData
  toggling: string
  settingDefault: string
  settingModel: string
  onSave: (config: ConfigData) => void
  onAdd: () => void
  onQuickSetup: (provider?: ProviderConfig) => void
  onClone: (provider: ProviderConfig) => void
  onDelete: (provider: ProviderConfig) => void | Promise<void>
  onToggle: (provider: ProviderConfig, enabled: boolean) => void | Promise<void>
  onSetDefault: (provider: ProviderConfig) => void | Promise<void>
  onSetDefaultModel: (provider: ProviderConfig, model: string) => void | Promise<void>
}

export function ProviderWorkbench(props: Props) {
  const { t } = useI18n()
  const { config } = props
  const defaultId = config.defaultProvider || config.provider
  const providers = config.providers.filter(
    (provider) => provider.type === 'chat' && (provider.configured || provider.custom),
  )
  const [selectedId, setSelectedId] = useState(defaultId)
  const [section, setSection] = useState<'connection' | 'models'>('connection')
  const [revision, setRevision] = useState(0)
  const selected =
    providers.find((provider) => provider.id === selectedId) ??
    providers.find((provider) => provider.id === defaultId) ??
    providers[0]
  const Icon = selected ? PROVIDER_ICONS[selected.id] || Server : Server
  const isDefault = selected?.id === defaultId
  const busy = Boolean(props.settingDefault || props.settingModel || props.toggling)
  return (
    <section
      data-config-card="models-connections"
      className="overflow-hidden rounded-xl border border-border bg-card"
    >
      <div
        data-model-provider-split-panel
        className="grid min-h-[36rem] grid-cols-[52px_minmax(0,1fr)] md:grid-cols-[208px_minmax(0,1fr)]"
      >
        <nav
          aria-label={t('config:configPage.connections')}
          className="min-w-0 border-r border-border p-2"
        >
          <div className="mb-3 flex h-8 items-center justify-between px-1 text-xs text-muted-foreground">
            <span className="max-md:sr-only">{t('config:configPage.connections')}</span>
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label={t('config:configPage.addCustomConnection')}
              title={t('config:configPage.addCustomConnection')}
              onClick={props.onAdd}
            >
              <Plus size={15} />
            </Button>
          </div>
          <div className="space-y-1">
            {providers.map((provider) => {
              const ProviderIcon = PROVIDER_ICONS[provider.id] || Server
              return (
                <button
                  key={provider.id}
                  type="button"
                  title={provider.name}
                  aria-label={provider.name}
                  aria-current={selected?.id === provider.id ? 'true' : undefined}
                  onClick={() => setSelectedId(provider.id)}
                  className={cn(
                    'flex h-9 w-full items-center gap-2 rounded-lg border px-2 text-left text-[13px] max-md:justify-center max-md:px-0',
                    selected?.id === provider.id
                      ? 'border-border bg-muted'
                      : 'border-transparent hover:bg-muted/60',
                  )}
                >
                  <ProviderIcon size={16} className="shrink-0" />
                  <span className="min-w-0 flex-1 truncate max-md:sr-only">{provider.name}</span>
                  <span
                    className={cn(
                      'size-1.5 shrink-0 rounded-full max-md:hidden',
                      provider.configured && provider.enabled
                        ? 'bg-emerald-500'
                        : 'bg-muted-foreground/40',
                    )}
                  />
                </button>
              )
            })}
          </div>
        </nav>
        <div className="min-w-0 p-4 sm:p-6">
          {selected ? (
            <>
              <div className="mb-5 flex min-w-0 items-center gap-2.5">
                <Icon size={22} className="shrink-0" />
                <div className="min-w-0 flex-1">
                  <h2 className="truncate text-base font-semibold">{selected.name}</h2>
                  <p className="truncate text-xs text-muted-foreground">
                    {isDefault ? t('config:configPage.defaultBadge') : selected.api}
                  </p>
                </div>
                <SettingsSwitch
                  ariaLabel={t('config:configPage.providerEnabled', { name: selected.name })}
                  value={selected.configured && selected.enabled}
                  disabled={!selected.configured || busy}
                  onChange={(enabled) => void props.onToggle(selected, enabled)}
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
                        isDefault ||
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
                    <DropdownMenuItem onSelect={() => props.onClone(selected)}>
                      <Copy size={14} />
                      {t('config:configPage.cloneProvider')}
                    </DropdownMenuItem>
                    {selected.custom && (
                      <DropdownMenuItem
                        className="text-destructive"
                        onSelect={() => void props.onDelete(selected)}
                      >
                        <Trash2 size={14} />
                        {t('config:configPage.deleteProvider')}
                      </DropdownMenuItem>
                    )}
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
              <div className="mb-5 flex items-center gap-4 border-b border-border">
                {(
                  [
                    { id: 'connection', label: t('config:providerWorkbench.connection') },
                    { id: 'models', label: t('config:providerWorkbench.models') },
                  ] as const
                ).map((item) => (
                  <button
                    type="button"
                    key={item.id}
                    aria-pressed={section === item.id}
                    className={cn(
                      'border-b-2 px-0.5 pb-2 text-[13px]',
                      section === item.id
                        ? 'border-foreground text-foreground'
                        : 'border-transparent text-muted-foreground',
                    )}
                    onClick={() => setSection(item.id)}
                  >
                    {item.label}
                    {item.id === 'models' && (
                      <span className="ml-1.5 text-xs text-muted-foreground">
                        {selected.models.length}
                      </span>
                    )}
                  </button>
                ))}
                <Button
                  variant="ghost"
                  size="sm"
                  className="mb-1 ml-auto h-7 px-1.5 text-xs"
                  onClick={() => props.onQuickSetup(selected)}
                >
                  <Wand2 size={13} />
                  {t('config:configPage.quickSetup')}
                </Button>
              </div>
              <div hidden={section !== 'connection'}>
                <ProviderConfigModal
                  key={`${selected.id}:${selected.defaultModel}:${selected.enabled}:${revision}`}
                  embedded
                  initialProvider={selected}
                  onClose={() => setRevision((value) => value + 1)}
                  onCreated={(data) => {
                    props.onSave(data)
                    setRevision((value) => value + 1)
                  }}
                />
              </div>
              {section === 'models' && (
                <div className="space-y-3">
                  <p className="text-xs leading-5 text-muted-foreground">
                    {t('config:providerWorkbench.modelsHint')}
                  </p>
                  {selected.models
                    .filter((model) => model.kind === 'chat')
                    .map((model) => (
                      <div
                        key={model.id}
                        className="flex min-w-0 items-center gap-3 rounded-lg border border-border px-3 py-2.5"
                      >
                        <div className="min-w-0 flex-1">
                          <p className="truncate text-[13px] font-medium">
                            {model.name || model.id}
                          </p>
                          <p className="truncate text-xs text-muted-foreground">{model.id}</p>
                        </div>
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          aria-label={`${t('config:configPage.providerDefaultModel')}: ${model.id}`}
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
                      </div>
                    ))}
                  <Button variant="outline" size="sm" onClick={() => props.onQuickSetup(selected)}>
                    <Plus size={14} />
                    {t('config:providerWorkbench.manageModels')}
                  </Button>
                </div>
              )}
            </>
          ) : (
            <div className="flex min-h-80 flex-col items-center justify-center gap-4 text-center">
              <Server size={28} className="text-muted-foreground" />
              <p className="text-sm">{t('config:configPage.noModelConfiguredYet')}</p>
              <Button onClick={() => props.onQuickSetup()}>
                {t('config:configPage.quickSetup')}
              </Button>
            </div>
          )}
        </div>
      </div>
    </section>
  )
}
