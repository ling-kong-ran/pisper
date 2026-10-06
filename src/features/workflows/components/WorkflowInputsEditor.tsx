import { useId, useState } from 'react'
import { ChevronDown, Pencil, Plus, Trash2 } from 'lucide-react'
import { AppSelect } from '@/components/common/AppSelect'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { WorkflowContentField } from '@/features/workflows/components/WorkflowContentField'
import {
  workflowInputDefault,
  workflowInputDefinitionError,
  workflowInputType,
} from '@/features/workflows/model/workflow-inputs'
import type { WorkflowInput } from '@/features/workflows/model/types'
import type { WorkflowTranslate } from '@/features/workflows/model/workflow-templates'

function inputTypeLabel(input: WorkflowInput, t: WorkflowTranslate) {
  if (input.type === 'text') return t('workflows:inputs.longText')
  if (input.type === 'number') return t('workflows:inputs.number')
  if (input.type === 'boolean') return t('workflows:inputs.boolean')
  if (input.type === 'image') return t('workflows:inputs.image')
  if (input.type === 'video') return t('workflows:inputs.video')
  return t('workflows:inputs.shortText')
}

export function WorkflowInputsEditor({
  inputs,
  onChange,
  t,
  onUploadBusy,
}: {
  inputs: WorkflowInput[]
  onChange: (inputs: WorkflowInput[]) => void
  t: WorkflowTranslate
  onUploadBusy?: (id: string, busy: boolean) => void
}) {
  const formId = useId()
  const [editingId, setEditingId] = useState<string | null>(null)
  const error = workflowInputDefinitionError(inputs, t)
  const update = (id: string, patch: Partial<WorkflowInput>) =>
    onChange(inputs.map((input) => (input.id === id ? { ...input, ...patch } : input)))
  const addInput = () => {
    let index = inputs.length + 1
    while (inputs.some((input) => input.name === `input_${index}`)) index += 1
    const id = crypto.randomUUID()
    onChange([
      ...inputs,
      {
        id,
        name: `input_${index}`,
        label: t('workflows:workflowsPage.newInput'),
        type: 'string',
        required: false,
        defaultValue: '',
        description: '',
      },
    ])
    setEditingId(id)
  }
  return (
    <section className="mt-4 min-w-0 space-y-3" aria-labelledby={`${formId}-title`}>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 id={`${formId}-title`} className="text-sm font-medium">
          {t('workflows:workflowsPage.inputParameters')}
        </h3>
        <Button variant="ghost" size="sm" disabled={inputs.length >= 30} onClick={addInput}>
          <Plus />
          {t('workflows:workflowsPage.addInput')}
        </Button>
      </div>
      <p className="text-xs leading-relaxed text-muted-foreground">
        {t('workflows:inputs.definitionHint')}
      </p>
      {error && (
        <p role="alert" className="text-xs text-destructive">
          {error}
        </p>
      )}
      {inputs.length === 0 && (
        <p className="text-xs text-muted-foreground">{t('workflows:inputs.emptyDefinition')}</p>
      )}
      {inputs.map((input) => (
        <Collapsible
          className="min-w-0 rounded-lg border"
          key={input.id}
          open={editingId === input.id}
          onOpenChange={(open) => setEditingId(open ? input.id : null)}
        >
          <div className="flex min-w-0 items-center gap-2 p-3">
            <div className="min-w-0 flex-1 space-y-1">
              <p className="truncate text-sm font-medium">{input.label || input.name}</p>
              <p className="text-xs text-muted-foreground">
                {inputTypeLabel(input, t)} ·{' '}
                {input.required
                  ? t('workflows:workflowsPage.required')
                  : t('workflows:inputs.optional')}
              </p>
            </div>
            <CollapsibleTrigger asChild>
              <Button
                variant="ghost"
                size="sm"
                aria-label={t('workflows:inputs.editNamedInput', {
                  name: input.label || input.name,
                })}
              >
                <Pencil />
                {t('workflows:inputs.editInput')}
              </Button>
            </CollapsibleTrigger>
          </div>
          <CollapsibleContent className="space-y-3 border-t p-3">
            <div className="space-y-1">
              <Label htmlFor={`${formId}-${input.id}-label`} className="text-xs">
                {t('workflows:inputs.label')}
              </Label>
              <Input
                id={`${formId}-${input.id}-label`}
                value={input.label}
                maxLength={120}
                onChange={(event) => update(input.id, { label: event.target.value })}
              />
            </div>
            <div className="space-y-1">
              <Label id={`${formId}-${input.id}-type`} className="text-xs">
                {t('workflows:workflowsPage.parameterType')}
              </Label>
              <AppSelect
                aria-labelledby={`${formId}-${input.id}-type`}
                value={input.type}
                onChange={(event) => {
                  const type = workflowInputType(event.target.value)
                  update(input.id, { type, defaultValue: type === 'boolean' ? false : '' })
                }}
              >
                <option value="string">{t('workflows:inputs.shortText')}</option>
                <option value="text">{t('workflows:inputs.longText')}</option>
                <option value="number">{t('workflows:inputs.number')}</option>
                <option value="boolean">{t('workflows:inputs.boolean')}</option>
                <option value="image">{t('workflows:inputs.image')}</option>
                <option value="video">{t('workflows:inputs.video')}</option>
              </AppSelect>
            </div>
            <div className="space-y-1">
              <Label htmlFor={`${formId}-${input.id}-description`} className="text-xs">
                {t('workflows:inputs.description')}
              </Label>
              <Textarea
                id={`${formId}-${input.id}-description`}
                value={input.description}
                maxLength={300}
                rows={2}
                className="min-h-14"
                onChange={(event) => update(input.id, { description: event.target.value })}
              />
            </div>
            <details className="group rounded-md border p-2">
              <summary className="flex cursor-pointer list-none items-center justify-between gap-2 text-xs font-medium [&::-webkit-details-marker]:hidden">
                {t('workflows:inputs.advancedSettings')}
                <ChevronDown className="size-3.5 transition-transform group-open:rotate-180" />
              </summary>
              <div className="space-y-3 pt-3">
                <div className="space-y-1">
                  <Label htmlFor={`${formId}-${input.id}-name`} className="text-xs">
                    {t('workflows:workflowsPage.parameterName')}
                  </Label>
                  <Input
                    id={`${formId}-${input.id}-name`}
                    value={input.name}
                    maxLength={80}
                    onChange={(event) => update(input.id, { name: event.target.value })}
                  />
                  <code className="block break-all text-xs text-muted-foreground">
                    {'{{inputs.' + input.name + '}}'}
                  </code>
                </div>
                <div className="space-y-1">
                  {input.type !== 'text' && input.type !== 'image' && input.type !== 'video' && (
                    <Label htmlFor={`${formId}-${input.id}-default`} className="text-xs">
                      {t('workflows:inputs.defaultValue')}
                    </Label>
                  )}
                  {input.type === 'boolean' ? (
                    <Switch
                      id={`${formId}-${input.id}-default`}
                      checked={workflowInputDefault(input) === true}
                      onCheckedChange={(defaultValue) => update(input.id, { defaultValue })}
                    />
                  ) : input.type === 'text' || input.type === 'image' || input.type === 'video' ? (
                    <WorkflowContentField
                      input={input}
                      value={input.defaultValue}
                      label={t('workflows:inputs.defaultValue')}
                      onBusyChange={onUploadBusy}
                      onChange={(defaultValue) => update(input.id, { defaultValue })}
                    />
                  ) : (
                    <Input
                      id={`${formId}-${input.id}-default`}
                      type={input.type === 'number' ? 'number' : 'text'}
                      value={String(workflowInputDefault(input))}
                      onChange={(event) => update(input.id, { defaultValue: event.target.value })}
                    />
                  )}
                </div>
                <div className="flex items-center justify-between gap-2">
                  <Label htmlFor={`${formId}-${input.id}-required`} className="text-xs">
                    {t('workflows:workflowsPage.required')}
                  </Label>
                  <Switch
                    id={`${formId}-${input.id}-required`}
                    checked={input.required}
                    onCheckedChange={(required) => update(input.id, { required })}
                  />
                </div>
              </div>
            </details>
            <Button
              variant="ghost"
              size="sm"
              aria-label={t('workflows:inputs.deleteInput', { name: input.label })}
              onClick={() => {
                onChange(inputs.filter((item) => item.id !== input.id))
                setEditingId(null)
              }}
            >
              <Trash2 />
              {t('workflows:inputs.removeInput')}
            </Button>
          </CollapsibleContent>
        </Collapsible>
      ))}
    </section>
  )
}
