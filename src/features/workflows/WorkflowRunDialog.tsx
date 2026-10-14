import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { LoaderCircle, Play } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { WorkflowContentField } from './WorkflowContentField'
import {
  workflowInputDefault,
  workflowRunInputError,
  workflowRunInputValues,
} from './workflow-inputs'
import type { WorkflowInputValues } from './workflow-inputs'
import { workflowImageRequestCount } from './workflow-templates'
import type { Workflow, WorkflowInput } from './types'

export function WorkflowRunDialog({
  workflow,
  onClose,
  onRun,
}: {
  workflow: Workflow
  onClose: () => void
  onRun: (inputs: WorkflowInputValues) => Promise<boolean>
}) {
  const { t } = useI18n()
  const formId = useId()
  const fallback: WorkflowInput = {
    id: 'task',
    name: 'task',
    label: t('workflows:inputs.task'),
    description: t('workflows:inputs.contextHint'),
    type: 'text',
    required: false,
    defaultValue: '',
  }
  const inputs = workflow.inputs.length ? workflow.inputs : [fallback]
  const [values, setValues] = useState<WorkflowInputValues>(() =>
    Object.fromEntries(inputs.map((input) => [input.name, workflowInputDefault(input)])),
  )
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [uploading, setUploading] = useState<string[]>([])
  const uploadingIds = useRef(new Set<string>())
  const latestValues = useRef(values)
  latestValues.current = values
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  const changeValue = (name: string, value: WorkflowInputValues[string]) => {
    const next = { ...latestValues.current, [name]: value }
    latestValues.current = next
    setValues(next)
  }
  const onUploadBusy = useCallback((id: string, pending: boolean) => {
    if (pending) uploadingIds.current.add(id)
    else uploadingIds.current.delete(id)
    if (mounted.current) setUploading([...uploadingIds.current])
  }, [])
  const submitting = useRef(false)
  const run = async () => {
    if (submitting.current || uploadingIds.current.size) return
    const submittedValues = latestValues.current
    const problem = workflowRunInputError(inputs, submittedValues, t)
    if (problem) {
      setError(problem)
      return
    }
    submitting.current = true
    setBusy(true)
    setError('')
    try {
      const started = await onRun(workflowRunInputValues(inputs, submittedValues))
      if (!mounted.current) return
      if (started) onClose()
      else setError(t('workflows:inputs.startFailed'))
    } catch {
      if (mounted.current) setError(t('workflows:inputs.startFailed'))
    } finally {
      submitting.current = false
      if (mounted.current) setBusy(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !submitting.current) onClose()
      }}
    >
      <DialogContent
        className="max-h-[85vh] overflow-y-auto supports-[height:100dvh]:max-h-[85dvh] sm:max-w-xl"
        showCloseButton={!busy}
      >
        <DialogHeader>
          <DialogTitle>{t('workflows:inputs.runTitle', { name: workflow.name })}</DialogTitle>
          <DialogDescription>{t('workflows:inputs.runHint')}</DialogDescription>
        </DialogHeader>
        <form
          className="space-y-4"
          onSubmit={(event) => {
            event.preventDefault()
            void run()
          }}
        >
          {workflowImageRequestCount(workflow) > 0 && (
            <p className="rounded-lg bg-muted p-3 text-sm">
              {t('workflows:imageNodes.requestEstimate', {
                count: workflowImageRequestCount(workflow),
              })}
            </p>
          )}
          {inputs.map((input) => (
            <div className="space-y-1.5" key={input.id}>
              {input.type !== 'text' && input.type !== 'image' && input.type !== 'video' && (
                <Label htmlFor={`${formId}-${input.id}`} className="flex items-center gap-1">
                  {input.label}
                  {input.required && (
                    <span className="text-destructive" aria-hidden="true">
                      *
                    </span>
                  )}
                </Label>
              )}
              {input.type === 'boolean' ? (
                <Switch
                  id={`${formId}-${input.id}`}
                  checked={values[input.name] === true}
                  disabled={busy}
                  onCheckedChange={(value) => changeValue(input.name, value)}
                />
              ) : input.type === 'text' || input.type === 'image' || input.type === 'video' ? (
                <WorkflowContentField
                  input={input}
                  value={values[input.name]}
                  disabled={busy}
                  label={`${input.label}${input.required ? ' *' : ''}`}
                  onBusyChange={onUploadBusy}
                  onChange={(value) => changeValue(input.name, value)}
                />
              ) : (
                <Input
                  id={`${formId}-${input.id}`}
                  type={input.type === 'number' ? 'number' : 'text'}
                  step={input.type === 'number' ? 'any' : undefined}
                  value={String(values[input.name] ?? '')}
                  required={input.required}
                  disabled={busy}
                  aria-describedby={
                    input.description ? `${formId}-${input.id}-description` : undefined
                  }
                  onChange={(event) => changeValue(input.name, event.target.value)}
                />
              )}
              {input.description && (
                <p
                  id={`${formId}-${input.id}-description`}
                  className="text-xs leading-relaxed text-muted-foreground"
                >
                  {input.description}
                </p>
              )}
            </div>
          ))}
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={busy}
              onClick={() => {
                if (!submitting.current) onClose()
              }}
            >
              {t('workflows:sprite.cancel')}
            </Button>
            <Button type="submit" disabled={busy || uploading.length > 0}>
              {busy ? <LoaderCircle className="animate-spin" /> : <Play />}
              {t('workflows:workflowsPage.run')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
