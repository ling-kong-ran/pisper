import '@/features/config/model/provider-messages'
// 独立模型草稿：保存失败不关闭，不通过全局配置接口修改会话策略。
import { useState, type FormEvent } from 'react'
import { Check, LoaderCircle } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog'
import { cn } from '@/lib/utils'
import { providerApi, type ModelOptionsDraft } from '@/features/config/api/provider-api'
import type {
  ConfigData,
  ProviderConfig,
  ProviderModel,
} from '@/features/config/model/config-types'
const levels = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
function Option({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string
  checked: boolean
  disabled?: boolean
  onChange: () => void
}) {
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={checked}
      disabled={disabled}
      onClick={onChange}
      className={cn(
        'inline-flex h-9 items-center gap-2 rounded-lg border px-3 text-sm transition-colors disabled:opacity-50',
        checked
          ? 'border-foreground/30 bg-muted text-foreground'
          : 'border-border hover:bg-muted/60',
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          'grid size-3.5 place-items-center rounded-[3px] border',
          checked && 'border-primary bg-primary text-primary-foreground',
        )}
      >
        {checked && <Check size={11} />}
      </span>
      {label}
    </button>
  )
}
export function ProviderModelEditor({
  provider,
  model,
  onSave,
  onClose,
}: {
  provider: ProviderConfig
  model?: ProviderModel
  onSave: (config: ConfigData) => void
  onClose: () => void
}) {
  const { t } = useI18n()
  const [draft, setDraft] = useState<ModelOptionsDraft>(() => ({
    modelId: model?.id || '',
    name: model?.name || '',
    capabilities: model?.capabilities || [
      model?.kind === 'image' ? 'image' : model?.kind === 'video' ? 'video' : 'chat',
    ],
    input: model?.input || ['text'],
    reasoning: model?.reasoning ?? true,
    contextWindow: model?.contextWindow || 200000,
    maxTokens: model?.maxTokens || 8192,
  }))
  const [selectedLevels, setSelectedLevels] = useState(
    model?.thinkingLevels || ['off', 'low', 'medium', 'high'],
  )
  const [levelsTouched, setLevelsTouched] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const chat = draft.capabilities.includes('chat')
  const save = async (event: FormEvent) => {
    event.preventDefault()
    if (saving) return
    setSaving(true)
    setError('')
    try {
      const result = await providerApi.saveModel(
        provider.id,
        {
          ...draft,
          modelId: draft.modelId.trim(),
          thinkingLevels: levelsTouched || !model ? selectedLevels : undefined,
        },
        !model,
      )
      onSave(result)
      onClose()
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !saving) onClose()
      }}
    >
      <DialogContent
        className="sm:max-w-[540px] p-5"
        showCloseButton={!saving}
        onEscapeKeyDown={(event) => {
          if (saving) event.preventDefault()
        }}
        onInteractOutside={(event) => event.preventDefault()}
      >
        <DialogHeader>
          <DialogTitle>
            {model ? t('providers:modelEditor.edit') : t('providers:modelEditor.add')}
          </DialogTitle>
          <DialogDescription>
            {provider.name} · {t('providers:modelEditor.description')}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={(event) => void save(event)} className="space-y-5">
          <fieldset disabled={saving} className="space-y-5">
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="grid gap-1.5 text-sm">
                {t('providers:modelEditor.id')}
                <Input
                  required
                  disabled={Boolean(model)}
                  value={draft.modelId}
                  onChange={(event) => setDraft({ ...draft, modelId: event.target.value })}
                />
              </label>
              <label className="grid gap-1.5 text-sm">
                {t('config:configPage.modelName')}
                <Input
                  value={draft.name}
                  onChange={(event) => setDraft({ ...draft, name: event.target.value })}
                />
              </label>
            </div>
            <fieldset className="space-y-2">
              <legend className="mb-2 text-sm font-medium">
                {t('providers:modelEditor.capabilities')}
              </legend>
              <div className="flex flex-wrap gap-2">
                {(['chat', 'image', 'video'] as const).map((capability) => (
                  <Option
                    key={capability}
                    label={
                      capability === 'chat'
                        ? t('providers:modelEditor.chat')
                        : capability === 'image'
                          ? t('providers:modelEditor.image')
                          : t('providers:modelEditor.video')
                    }
                    checked={draft.capabilities.includes(capability)}
                    onChange={() =>
                      setDraft({
                        ...draft,
                        capabilities: draft.capabilities.includes(capability)
                          ? draft.capabilities.filter((value) => value !== capability)
                          : [...draft.capabilities, capability],
                      })
                    }
                  />
                ))}
              </div>
              <p className="text-xs leading-relaxed text-muted-foreground">
                {t('providers:modelEditor.capabilitiesHint')}
              </p>
            </fieldset>
            {chat && (
              <fieldset className="space-y-2">
                <legend className="mb-2 text-sm font-medium">
                  {t('providers:modelEditor.chatCapabilities')}
                </legend>
                <div className="flex flex-wrap gap-2">
                  <Option
                    label={t('providers:modelEditor.vision')}
                    checked={draft.input.includes('image')}
                    onChange={() =>
                      setDraft({
                        ...draft,
                        input: draft.input.includes('image') ? ['text'] : ['text', 'image'],
                      })
                    }
                  />
                  <Option
                    label={t('providers:modelEditor.reasoning')}
                    checked={draft.reasoning}
                    onChange={() => setDraft({ ...draft, reasoning: !draft.reasoning })}
                  />
                </div>
              </fieldset>
            )}
            {chat && draft.reasoning && (
              <fieldset>
                <legend className="mb-2 text-sm font-medium">
                  {t('config:configPage.modelThinkingLevels')}
                </legend>
                <div className="flex flex-wrap gap-2">
                  {levels.map((level) => (
                    <Option
                      key={level}
                      label={level}
                      checked={level === 'off' || selectedLevels.includes(level)}
                      disabled={level === 'off'}
                      onChange={() => {
                        setLevelsTouched(true)
                        setSelectedLevels(
                          selectedLevels.includes(level)
                            ? selectedLevels.filter((value) => value !== level)
                            : [...selectedLevels, level],
                        )
                      }}
                    />
                  ))}
                </div>
              </fieldset>
            )}
            {chat && (
              <div className="grid gap-3 sm:grid-cols-2">
                {(['contextWindow', 'maxTokens'] as const).map((field) => (
                  <label key={field} className="grid gap-1.5 text-sm">
                    {field === 'contextWindow'
                      ? t('providers:modelEditor.contextWindow')
                      : t('providers:modelEditor.maxTokens')}
                    <Input
                      required
                      type="number"
                      min={1}
                      max={100000000}
                      step={1}
                      value={draft[field]}
                      onChange={(event) =>
                        setDraft({ ...draft, [field]: Number(event.target.value) })
                      }
                    />
                  </label>
                ))}
              </div>
            )}
          </fieldset>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" disabled={saving} onClick={onClose}>
              {t('config:configPage.cancel')}
            </Button>
            <Button
              type="submit"
              disabled={saving || !draft.capabilities.length || !draft.modelId.trim()}
            >
              {saving && <LoaderCircle className="animate-spin" size={14} />}{' '}
              {saving ? t('config:configPage.saving') : t('config:configPage.saveChanges')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
