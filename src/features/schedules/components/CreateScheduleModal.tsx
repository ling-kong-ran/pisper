// 新建定时任务弹窗：提交名称、执行目标和工作目录。
import { useRef, useState } from 'react'
import type { FormEvent } from 'react'
import { AlertTriangle, Plus, RefreshCw, X } from 'lucide-react'
import { useI18n } from '@/app/i18n/use-i18n'
import { AppCardHeader, AppError } from '@/components/ui/app-primitives'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { Dialog, DialogContent, DialogTitle, DialogDescription } from '@/components/ui/dialog'
import { apiJson } from '@/lib/http/api'
import { ScheduleExecutionModeField } from './ScheduleExecutionModeField'
import { ScheduleTargetFields } from './ScheduleTargetFields'
import { ScheduleWorkspaceField } from './ScheduleWorkspaceField'
import { scheduleTargetValid } from '@/features/schedules/model/schedule-utils'
import type {
  ScheduleExecutionMode,
  ScheduleMutationResult,
  ScheduleTargetType,
  ScheduleWorkflow,
  NotificationTargets,
} from '@/features/schedules/model/schedule-types'

export function CreateScheduleModal({
  notificationTargets,
  workflows,
  defaultCwd,
  onClose,
  onCreated,
}: {
  notificationTargets: NotificationTargets
  workflows: ScheduleWorkflow[]
  defaultCwd: string
  onClose: () => void
  onCreated: (result: ScheduleMutationResult) => void
}) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [targetType, setTargetType] = useState<ScheduleTargetType>('prompt')
  const [prompt, setPrompt] = useState('')
  const [workflowId, setWorkflowId] = useState('')
  const [workflowInputs, setWorkflowInputs] = useState<Record<string, unknown>>({})
  const [cwd, setCwd] = useState(defaultCwd || '')
  const [executionMode, setExecutionMode] = useState<ScheduleExecutionMode>('full-access')
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)
  const nameRef = useRef<HTMLInputElement>(null)
  const [returnFocus] = useState(() =>
    document.activeElement instanceof HTMLElement ? document.activeElement : null,
  )
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (
      savingRef.current ||
      !name.trim() ||
      !scheduleTargetValid(targetType, prompt, workflowId, workflowInputs, workflows)
    )
      return
    savingRef.current = true
    setSaving(true)
    setError('')
    try {
      const notifications = Object.entries(notificationTargets)
        .filter(([, value]) => value.enabled)
        .map(([id]) => id)
      onCreated(
        await apiJson<ScheduleMutationResult>('/api/schedules', {
          method: 'POST',
          body: JSON.stringify({
            name,
            targetType,
            prompt,
            workflowId,
            workflowInputs,
            cwd,
            enabled: true,
            frequency: 'daily',
            time: '09:00',
            timezone: 'Asia/Hong_Kong',
            executionMode,
            notifications,
            notifyOn: 'always',
          }),
        }),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      savingRef.current = false
      setSaving(false)
    }
  }
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !savingRef.current) onClose()
      }}
    >
      <DialogContent
        className="z-[71] block bg-[var(--solid)] p-[18px] sm:max-w-[430px]"
        overlayClassName="z-[70] bg-[var(--modal-overlay)] backdrop-blur-sm"
        showCloseButton={false}
        onOpenAutoFocus={(event) => {
          event.preventDefault()
          nameRef.current?.focus()
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault()
          returnFocus?.focus()
        }}
      >
        <form className="space-y-3" onSubmit={submit}>
          <AppCardHeader>
            <div>
              <DialogTitle>{t('schedules:schedulesPage.newScheduledTask')}</DialogTitle>
              <DialogDescription>
                {t(
                  'schedules:schedulesPage.youCanContinueSettingTheRunTimeAndNotificationChannelsAfterCreation',
                )}
              </DialogDescription>
            </div>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              aria-label={t('schedules:schedulesPage.closeDialog')}
              disabled={saving}
              onClick={onClose}
            >
              <X size={17} />
            </Button>
          </AppCardHeader>
          <FieldLabel variant="control">
            {t('schedules:schedulesPage.taskName')}
            <input
              ref={nameRef}
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder={t('schedules:schedulesPage.forExampleDailyCodeReview')}
            />
          </FieldLabel>
          <ScheduleTargetFields
            targetType={targetType}
            prompt={prompt}
            workflowId={workflowId}
            workflowInputs={workflowInputs}
            workflows={workflows}
            onChange={(patch) => {
              if (patch.targetType) setTargetType(patch.targetType)
              if (patch.prompt !== undefined) setPrompt(patch.prompt)
              if (patch.workflowId !== undefined) setWorkflowId(patch.workflowId)
              if (patch.workflowInputs !== undefined) setWorkflowInputs(patch.workflowInputs)
            }}
          />
          {targetType === 'prompt' && (
            <>
              <ScheduleWorkspaceField value={cwd} onChange={setCwd} />
              <ScheduleExecutionModeField value={executionMode} onChange={setExecutionMode} />
            </>
          )}
          {error && (
            <AppError>
              <AlertTriangle size={13} />
              {error}
            </AppError>
          )}
          <div className="flex justify-end gap-[8px] [margin-top:18px]">
            <Button
              type="button"
              variant="outline"
              size="lg"
              className="bg-surface-subtle"
              disabled={saving}
              onClick={onClose}
            >
              {t('schedules:schedulesPage.cancel')}
            </Button>
            <Button
              size="lg"
              disabled={
                saving ||
                !name.trim() ||
                !scheduleTargetValid(targetType, prompt, workflowId, workflowInputs, workflows)
              }
            >
              {saving ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
              {saving
                ? t('schedules:schedulesPage.creating')
                : t('schedules:schedulesPage.createTask')}
            </Button>
          </div>
        </form>
      </DialogContent>
    </Dialog>
  )
}
