// 新建定时任务面板：名称、频率、时区和执行目标完整配置。
import { useState } from 'react'
import { AlertTriangle, Plus, RefreshCw } from 'lucide-react'
import { AppCard as Panel, AppCardHeader, AppError } from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/common/AppSelect'
import { useI18n } from '@/app/i18n/use-i18n'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { apiJson } from '@/lib/http/api'
import { ScheduleExecutionModeField } from './ScheduleExecutionModeField'
import { ScheduleTargetFields } from './ScheduleTargetFields'
import { ScheduleWorkspaceField } from './ScheduleWorkspaceField'
import {
  FREQUENCIES,
  INTERVAL_UNITS,
  TIMEZONES,
} from '@/features/schedules/model/schedule-constants'
import {
  frequencyLabel,
  intervalUnitLabel,
  scheduleTargetValid,
} from '@/features/schedules/model/schedule-utils'
import type {
  IntervalUnit,
  NotificationTargets,
  ScheduleExecutionMode,
  ScheduleFrequency,
  ScheduleMutationResult,
  ScheduleTargetType,
  ScheduleWorkflow,
} from '@/features/schedules/model/schedule-types'

export function CreateSchedulePanel({
  notificationTargets,
  workflows,
  defaultCwd,
  onCreated,
}: {
  notificationTargets: NotificationTargets
  workflows: ScheduleWorkflow[]
  defaultCwd: string
  onCreated: (result: ScheduleMutationResult) => void
}) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [targetType, setTargetType] = useState<ScheduleTargetType>('prompt')
  const [prompt, setPrompt] = useState('')
  const [workflowId, setWorkflowId] = useState('')
  const [workflowInputs, setWorkflowInputs] = useState<Record<string, unknown>>({})
  const [cwd, setCwd] = useState(defaultCwd || '')
  const [frequency, setFrequency] = useState<ScheduleFrequency>('daily')
  const [time, setTime] = useState('09:00')
  const [timezone, setTimezone] = useState('Asia/Hong_Kong')
  const [intervalValue, setIntervalValue] = useState(1)
  const [intervalUnit, setIntervalUnit] = useState<IntervalUnit>('hours')
  const [executionMode, setExecutionMode] = useState<ScheduleExecutionMode>('full-access')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  // 创建新任务（带工作目录草稿）。
  const create = async () => {
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
            frequency,
            time,
            timezone,
            intervalValue,
            intervalUnit,
            executionMode,
            notifications,
            notifyOn: 'always',
          }),
        }),
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }
  return (
    <Panel>
      <AppCardHeader>
        <div>
          <h2>{t('schedules:schedulesPage.newScheduledTask')}</h2>
          <p>
            {t(
              'schedules:schedulesPage.youCanContinueEditingTheRunTimeAndNotificationChannelsAfterCreation',
            )}
          </p>
        </div>
      </AppCardHeader>
      <FieldLabel variant="control">
        {t('schedules:schedulesPage.taskName')}
        <input
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
      {targetType === 'prompt' && <ScheduleWorkspaceField value={cwd} onChange={setCwd} />}
      <div className="form-grid grid gap-[9px] three [.form-grid&]:grid-cols-[repeat(3,minmax(0,1fr))] max-[650px]:[.form-grid&]:grid-cols-[1fr]">
        <FieldLabel variant="control">
          {t('schedules:schedulesPage.frequency')}
          <AppSelect
            value={frequency}
            onChange={(event) => setFrequency(event.target.value as ScheduleFrequency)}
          >
            {Object.keys(FREQUENCIES).map((value) => (
              <option value={value} key={value}>
                {frequencyLabel(value as ScheduleFrequency, t)}
              </option>
            ))}
          </AppSelect>
        </FieldLabel>
        {frequency === 'interval' ? (
          <FieldLabel variant="control">
            {t('schedules:schedulesPage.runInterval')}
            <span className="schedule-interval-input [&_input]:w-full [&_input]:min-w-0 [&_input]:h-full [&_input]:border-0 [&_input]:[outline:0] [&_input]:bg-transparent [&_input]:p-[0_9px] [&_input]:text-[var(--text)] [&_select]:h-full [&_select]:border-0 [&_select]:[border-left:1px_solid_var(--stroke)] [&_select]:[outline:0] [&_select]:bg-[var(--solid)] [&_select]:p-[0_8px] [&_select]:text-[var(--text-tertiary)] [&_select]:text-[12px] [&_[data-slot='select-trigger']]:w-auto [&_[data-slot='select-trigger']]:min-w-[74px] [&_[data-slot='select-trigger']]:[border-left:1px_solid_var(--stroke)] [&_[data-slot='select-trigger']]:rounded-[0] [&_[data-slot='select-trigger']]:bg-[var(--solid)] [&_[data-slot='select-trigger']]:text-[var(--text-tertiary)] grid h-[31px] grid-cols-[minmax(0,1fr)_auto] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--surface-subtle)]">
              <input
                type="number"
                min="1"
                value={intervalValue}
                onChange={(event) => setIntervalValue(Number(event.target.value))}
              />
              <AppSelect
                value={intervalUnit}
                onChange={(event) => setIntervalUnit(event.target.value as IntervalUnit)}
              >
                {Object.keys(INTERVAL_UNITS).map((value) => (
                  <option value={value} key={value}>
                    {intervalUnitLabel(value as IntervalUnit, t)}
                  </option>
                ))}
              </AppSelect>
            </span>
          </FieldLabel>
        ) : (
          <FieldLabel variant="control">
            {t('schedules:schedulesPage.time')}
            <input type="time" value={time} onChange={(event) => setTime(event.target.value)} />
          </FieldLabel>
        )}
        <FieldLabel variant="control">
          {t('schedules:schedulesPage.timeZone')}
          <AppSelect value={timezone} onChange={(event) => setTimezone(event.target.value)}>
            {TIMEZONES.map((item) => (
              <option value={item} key={item}>
                {item}
              </option>
            ))}
          </AppSelect>
        </FieldLabel>
        {targetType === 'prompt' && (
          <ScheduleExecutionModeField value={executionMode} onChange={setExecutionMode} />
        )}
      </div>
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
      <div className="form-footer [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] flex items-center justify-between gap-[10px] [margin-top:10px]">
        <span>{t('schedules:schedulesPage.enabledAutomaticallyAfterCreation')}</span>
        <Button
          size="lg"
          disabled={
            saving ||
            !name.trim() ||
            !scheduleTargetValid(targetType, prompt, workflowId, workflowInputs, workflows)
          }
          onClick={create}
        >
          {saving ? <RefreshCw className="animate-spin" size={14} /> : <Plus size={14} />}
          {saving ? t('schedules:schedulesPage.creating') : t('schedules:schedulesPage.createTask')}
        </Button>
      </div>
    </Panel>
  )
}
