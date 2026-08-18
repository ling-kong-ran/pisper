// 定时计划页：查看/创建/启停周期任务（如每日对话/工作流运行），
// 展示下次执行时间与历史。
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Bell,
  Bot,
  CheckCircle2,
  FolderOpen,
  MessageCircle,
  Play,
  Plus,
  RefreshCw,
  Trash2,
  X,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
  StatusBadge as Badge,
  AppCardHeader,
  AppError,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/AppSelect'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { useI18n } from '@/app/use-i18n'
import { StarOrbit } from '@/components/StarOrbit'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { hasSystemDirectoryPicker, pickSystemDirectory } from '@/lib/pick-system-directory'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { FormEvent } from 'react'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

type NotificationTarget = 'browser' | 'feishu' | 'weixin'
type ScheduleFrequency = 'interval' | 'daily' | 'weekly' | 'monthly'
type IntervalUnit = 'minutes' | 'hours' | 'days'
type ScheduleExecutionMode = 'full-access' | 'read-only'
type ScheduleTargetType = 'prompt' | 'workflow'
type ScheduleStatus = 'idle' | 'running' | 'completed' | 'failed' | 'interrupted'
type ScheduleWorkflowInput = {
  id: string
  name: string
  label: string
  type: 'string' | 'text' | 'number' | 'boolean'
  required: boolean
  defaultValue: unknown
  description?: string
}
type ScheduleWorkflow = {
  id: string
  name: string
  description: string
  revision: number
  inputs: ScheduleWorkflowInput[]
}
type ScheduleTask = {
  id: string
  name: string
  targetType: ScheduleTargetType
  prompt: string
  workflowId: string
  workflowInputs: Record<string, unknown>
  enabled: boolean
  frequency: ScheduleFrequency
  intervalValue: number
  intervalUnit: IntervalUnit
  time: string
  timezone: string
  dayOfWeek: number
  dayOfMonth: number
  cwd: string
  executionMode: ScheduleExecutionMode
  model: { provider: string; model: string } | null
  notifications: NotificationTarget[]
  notifyOn: 'always' | 'failure'
  nextRunAt?: string | null
  lastRunAt?: string | null
  lastStatus: ScheduleStatus
}
type ScheduleDraft = Pick<
  ScheduleTask,
  | 'id'
  | 'name'
  | 'targetType'
  | 'prompt'
  | 'workflowId'
  | 'workflowInputs'
  | 'enabled'
  | 'frequency'
  | 'intervalValue'
  | 'intervalUnit'
  | 'time'
  | 'timezone'
  | 'dayOfWeek'
  | 'dayOfMonth'
  | 'cwd'
  | 'executionMode'
  | 'model'
  | 'notifications'
  | 'notifyOn'
>
type ScheduleRun = {
  id: string
  taskId: string
  trigger: 'manual' | 'scheduled'
  status: 'running' | 'completed' | 'failed' | 'interrupted'
  startedAt: string
  durationMs: number
  summary?: string
  error?: string
  workflowRunId?: string
}
type NotificationTargets = Record<NotificationTarget, { enabled: boolean }>
type SchedulesData = {
  tasks: ScheduleTask[]
  runs: ScheduleRun[]
  notificationTargets: NotificationTargets
  workflows: ScheduleWorkflow[]
  defaultCwd: string
}
type ScheduleMutationResult = { task: ScheduleTask; state: SchedulesData }
type ScheduleExecutionModeFieldProps = {
  value: ScheduleExecutionMode
  onChange: (value: ScheduleExecutionMode) => void
}
type ScheduleWorkspaceFieldProps = {
  value: string
  onChange: (value: string) => void
}
type ScheduleCreatorProps = {
  notificationTargets: NotificationTargets
  workflows: ScheduleWorkflow[]
  defaultCwd: string
  onCreated: (result: ScheduleMutationResult) => void
}
type CreateScheduleModalProps = ScheduleCreatorProps & { onClose: () => void }
type SchedulesPageProps = {
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  openNotificationSettings: () => void
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

const TARGETS = {
  browser: { name: '通知', Icon: Bell },
  feishu: { name: '飞书', Icon: Bot },
  weixin: { name: '微信', Icon: MessageCircle },
}
const FREQUENCIES = { interval: '每隔一段时间', daily: '每天', weekly: '每周', monthly: '每月' }
const INTERVAL_UNITS = { minutes: '分钟', hours: '小时', days: '天' }
const TIMEZONES = [
  ...new Set([
    'Asia/Hong_Kong',
    'UTC',
    ...(typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : []),
  ]),
]
const SCHEDULE_EXECUTION_MODES: ScheduleExecutionMode[] = ['full-access', 'read-only']

function executionModeHelp(mode: ScheduleExecutionMode, t: ReturnType<typeof useI18n>['t']) {
  if (mode === 'full-access') return t('schedules:schedulesPage.fullAccessHelp')
  return t('schedules:schedulesPage.readOnlyHelp')
}

function frequencyLabel(frequency: ScheduleFrequency, t: ReturnType<typeof useI18n>['t']) {
  if (frequency === 'daily') return t('schedules:schedulesPage.daily')
  if (frequency === 'weekly') return t('schedules:schedulesPage.weekly')
  if (frequency === 'monthly') return t('schedules:schedulesPage.monthly')
  return t('schedules:schedulesPage.atIntervals')
}

function intervalUnitLabel(unit: IntervalUnit, t: ReturnType<typeof useI18n>['t']) {
  if (unit === 'hours') return t('schedules:schedulesPage.hours')
  if (unit === 'days') return t('schedules:schedulesPage.days')
  return t('schedules:schedulesPage.minutes')
}

function notificationTargetLabel(target: NotificationTarget, t: ReturnType<typeof useI18n>['t']) {
  if (target === 'feishu') return t('schedules:schedulesPage.feishu')
  if (target === 'weixin') return t('schedules:schedulesPage.weChat')
  return t('schedules:schedulesPage.browserNotification')
}

function ScheduleExecutionModeField({ value, onChange }: ScheduleExecutionModeFieldProps) {
  const { t } = useI18n()
  return (
    <FieldLabel variant="control">
      {t('schedules:schedulesPage.executionMode')}
      <AppSelect
        value={value}
        onChange={(event) => onChange(event.target.value as ScheduleExecutionMode)}
      >
        {SCHEDULE_EXECUTION_MODES.map((mode) => (
          <option value={mode} key={mode}>
            {mode === 'full-access'
              ? t('schedules:schedulesPage.fullAccess')
              : t('schedules:schedulesPage.readOnly')}
          </option>
        ))}
      </AppSelect>
      <small>{executionModeHelp(value, t)}</small>
    </FieldLabel>
  )
}

function workflowInputDefaults(workflow?: ScheduleWorkflow) {
  return Object.fromEntries(
    (workflow?.inputs || []).map((input) => [input.name, input.defaultValue ?? '']),
  )
}

function scheduleTargetValid(
  targetType: ScheduleTargetType,
  prompt: string,
  workflowId: string,
  workflowInputs: Record<string, unknown>,
  workflows: ScheduleWorkflow[],
) {
  if (targetType === 'prompt') return Boolean(prompt.trim())
  const workflow = workflows.find((item) => item.id === workflowId)
  if (!workflow) return false
  return workflow.inputs.every((input) => {
    const value = Object.hasOwn(workflowInputs, input.name)
      ? workflowInputs[input.name]
      : input.defaultValue
    return !input.required || (value !== undefined && value !== null && value !== '')
  })
}

function ScheduleTargetFields({
  targetType,
  prompt,
  workflowId,
  workflowInputs,
  workflows,
  onChange,
}: {
  targetType: ScheduleTargetType
  prompt: string
  workflowId: string
  workflowInputs: Record<string, unknown>
  workflows: ScheduleWorkflow[]
  onChange: (patch: {
    targetType?: ScheduleTargetType
    prompt?: string
    workflowId?: string
    workflowInputs?: Record<string, unknown>
  }) => void
}) {
  const { t } = useI18n()
  const workflow = workflows.find((item) => item.id === workflowId)
  return (
    <div className="grid gap-[10px]">
      <FieldLabel variant="control">
        {t('schedules:schedulesPage.executionTarget')}
        <AppSelect
          value={targetType}
          onChange={(event) => {
            const nextTarget = event.target.value as ScheduleTargetType
            const nextWorkflow = workflows[0]
            onChange(
              nextTarget === 'workflow'
                ? {
                    targetType: nextTarget,
                    workflowId: workflowId || nextWorkflow?.id || '',
                    workflowInputs: workflowId
                      ? workflowInputs
                      : workflowInputDefaults(nextWorkflow),
                  }
                : { targetType: nextTarget },
            )
          }}
        >
          <option value="prompt">Prompt</option>
          <option value="workflow">{t('schedules:schedulesPage.workflow')}</option>
        </AppSelect>
      </FieldLabel>
      {targetType === 'prompt' ? (
        <FieldLabel variant="control">
          Prompt
          <textarea
            value={prompt}
            onChange={(event) => onChange({ prompt: event.target.value })}
            placeholder={t('schedules:schedulesPage.describeTheWorkTheAgentShouldCompleteEachTime')}
          />
        </FieldLabel>
      ) : (
        <>
          <FieldLabel variant="control">
            {t('schedules:schedulesPage.workflow')}
            <AppSelect
              value={workflowId}
              onChange={(event) => {
                const nextWorkflow = workflows.find((item) => item.id === event.target.value)
                onChange({
                  workflowId: event.target.value,
                  workflowInputs: workflowInputDefaults(nextWorkflow),
                })
              }}
            >
              <option value="">{t('schedules:schedulesPage.selectPublishedWorkflow')}</option>
              {workflows.map((item) => (
                <option value={item.id} key={item.id}>
                  {item.name} · v{item.revision}
                </option>
              ))}
            </AppSelect>
            <small>
              {workflow?.description ||
                (workflows.length
                  ? t('schedules:schedulesPage.workflowUsesItsOwnRuntimeSettings')
                  : t('schedules:schedulesPage.noPublishedWorkflows'))}
            </small>
          </FieldLabel>
          {workflow?.inputs.length ? (
            <div className="grid gap-[8px]">
              <div className="schedule-workflow-inputs-heading [&_strong]:text-[12px] [&_small]:text-[var(--text-muted)] [&_small]:text-[11px] [&_small]:text-right max-[650px]:items-start max-[650px]:flex-col max-[650px]:gap-[2px] max-[650px]:[&_small]:text-left flex [align-items:baseline] justify-between gap-[12px]">
                <strong>{t('schedules:schedulesPage.workflowInputs')}</strong>
                <small>{t('schedules:schedulesPage.workflowInputsHelp')}</small>
              </div>
              <div className="schedule-workflow-inputs [&_input[type='checkbox']]:w-[16px] [&_input[type='checkbox']]:h-[16px] [&_input[type='checkbox']]:[justify-self:end] max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(2,minmax(0,1fr))] gap-[8px]">
                {workflow.inputs.map((input) => (
                  <FieldLabel
                    variant="control"
                    className={
                      input.type === 'boolean'
                        ? 'grid grid-cols-[minmax(0,1fr)_auto] items-center'
                        : undefined
                    }
                    key={input.id}
                  >
                    {input.label}
                    {input.required ? ' *' : ''}
                    {input.type === 'boolean' ? (
                      <input
                        type="checkbox"
                        checked={Boolean(workflowInputs[input.name])}
                        onChange={(event) =>
                          onChange({
                            workflowInputs: {
                              ...workflowInputs,
                              [input.name]: event.target.checked,
                            },
                          })
                        }
                      />
                    ) : (
                      <input
                        type={input.type === 'number' ? 'number' : 'text'}
                        required={input.required}
                        value={String(workflowInputs[input.name] ?? '')}
                        onChange={(event) =>
                          onChange({
                            workflowInputs: {
                              ...workflowInputs,
                              [input.name]: event.target.value,
                            },
                          })
                        }
                      />
                    )}
                    {input.description && <small>{input.description}</small>}
                  </FieldLabel>
                ))}
              </div>
            </div>
          ) : null}
        </>
      )}
    </div>
  )
}

function ScheduleWorkspaceField({ value, onChange }: ScheduleWorkspaceFieldProps) {
  const { t } = useI18n()
  const [pickerError, setPickerError] = useState('')
  const [webPickerOpen, setWebPickerOpen] = useState(false)

  // 浏览选择工作目录：桌面环境用系统选择器，否则退回 Web 输入对话框。
  const browse = async () => {
    setPickerError('')
    if (!hasSystemDirectoryPicker()) {
      setWebPickerOpen(true)
      return
    }
    try {
      const selected = await pickSystemDirectory(value)
      if (selected) onChange(selected)
    } catch (error) {
      setPickerError(error instanceof Error ? error.message : String(error))
    }
  }

  return (
    <>
      <FieldLabel variant="control">
        {t('schedules:schedulesPage.workingDirectory')}
        <span className="schedule-workspace-input [&_input]:min-w-0 [&_input]:font-[ui-monospace,_SFMono-Regular,_Consolas,_'Liberation_Mono',_monospace] grid grid-cols-[minmax(0,1fr)_auto] gap-[6px]">
          <input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder={t('schedules:schedulesPage.enterTheProjectSAbsolutePath')}
          />
          <Button
            type="button"
            variant="outline"
            size="lg"
            className="bg-surface-subtle"
            onClick={() => void browse()}
          >
            <FolderOpen size={13} />
            {t('schedules:schedulesPage.browseDirectories')}
          </Button>
        </span>
        <small>{t('schedules:schedulesPage.theScheduledAgentWillRunInThisDirectory')}</small>
      </FieldLabel>
      {pickerError && (
        <AppError>
          <AlertTriangle size={13} />
          {pickerError}
        </AppError>
      )}
      <WorkspacePicker
        open={webPickerOpen}
        initialPath={value}
        description={t('common:workspacePicker.selectWorkspaceForSchedule')}
        onOpenChange={setWebPickerOpen}
        onSelect={(selected) => onChange(selected)}
      />
    </>
  )
}

function taskDraft(task: ScheduleTask): ScheduleDraft {
  return {
    id: task.id,
    name: task.name,
    targetType: task.targetType || 'prompt',
    prompt: task.prompt,
    workflowId: task.workflowId || '',
    workflowInputs: task.workflowInputs || {},
    enabled: task.enabled,
    frequency: task.frequency,
    intervalValue: task.intervalValue || 1,
    intervalUnit: task.intervalUnit || 'hours',
    time: task.time,
    timezone: task.timezone,
    dayOfWeek: task.dayOfWeek,
    dayOfMonth: task.dayOfMonth,
    cwd: task.cwd,
    executionMode: task.executionMode || 'full-access',
    model: task.model,
    notifications: task.notifications || [],
    notifyOn: task.notifyOn || 'always',
  }
}

function nextRunLabel(task: ScheduleTask, locale = 'zh-CN') {
  if (!task.enabled || !task.nextRunAt) return locale === 'en-US' ? 'Paused' : '已暂停'
  return new Intl.DateTimeFormat(locale, {
    timeZone: task.timezone,
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(task.nextRunAt))
}

export function SchedulesPage({
  notify,
  registerPrimaryAction,
  openNotificationSettings,
  requestConfirm,
}: SchedulesPageProps) {
  const { t, language } = useI18n()
  const [data, setData] = useState<SchedulesData>({
    tasks: [],
    runs: [],
    notificationTargets: {
      browser: { enabled: false },
      feishu: { enabled: false },
      weixin: { enabled: false },
    },
    workflows: [],
    defaultCwd: '',
  })
  const [selectedId, setSelectedId] = useState('')
  const [draft, setDraft] = useState<ScheduleDraft | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [createOpen, setCreateOpen] = useState(false)
  usePagePrimaryAction(registerPrimaryAction, () => setCreateOpen(true))

  // 加载计划任务与运行记录；选中项失效时回退到第一个任务。
  const load = useCallback(async () => {
    try {
      const result = await apiJson<SchedulesData>('/api/schedules')
      setData(result)
      setSelectedId((current) =>
        result.tasks.some((task) => task.id === current) ? current : result.tasks[0]?.id || '',
      )
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    load()
  }, [load])
  useEffect(() => {
    const timer = window.setInterval(
      load,
      data.tasks.some((task) => task.lastStatus === 'running') ? 2000 : 10_000,
    )
    return () => window.clearInterval(timer)
  }, [data.tasks, load])

  const selected = data.tasks.find((task) => task.id === selectedId)
  const availableWorkflows = data.workflows || []
  useEffect(() => {
    setDraft((current) =>
      selected ? (current?.id === selected.id ? current : taskDraft(selected)) : null,
    )
  }, [selected])
  const runs = useMemo(
    () =>
      data.runs
        .filter((run) => run.taskId === selectedId)
        .sort((a, b) => new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime())
        .slice(0, 20),
    [data.runs, selectedId],
  )
  const updateDraft = (patch: Partial<ScheduleDraft>) =>
    setDraft((current) => (current ? { ...current, ...patch } : current))
  const toggleNotification = (target: NotificationTarget) => {
    if (!draft) return
    updateDraft({
      notifications: draft.notifications.includes(target)
        ? draft.notifications.filter((item) => item !== target)
        : [...draft.notifications, target],
    })
  }

  // 保存任务编辑：PATCH 到运行时并回显最新状态。
  const save = async () => {
    if (!selected || !draft) return
    setSaving(true)
    setError('')
    try {
      const result = await apiJson<ScheduleMutationResult>(
        `/api/schedules/${encodeURIComponent(selected.id)}`,
        {
          method: 'PATCH',
          body: JSON.stringify(draft),
        },
      )
      setData(result.state)
      setDraft(taskDraft(result.task))
      notify(t('schedules:schedulesPage.scheduledTaskSaved'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  // 立即运行任务：POST run 后刷新列表确认状态。
  const run = async () => {
    if (!selected) return
    setSaving(true)
    setError('')
    try {
      await apiJson(`/api/schedules/${encodeURIComponent(selected.id)}/run`, {
        method: 'POST',
        body: '{}',
      })
      await load()
      notify(t('schedules:schedulesPage.scheduledTaskStarted'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setSaving(false)
    }
  }

  // 删除任务（确认后），成功后清除选中。
  const remove = async () => {
    if (!selected) return
    const approved = await requestConfirm({
      title: t('schedules:schedulesPage.deleteScheduledTask'),
      message: t('schedules:schedulesPage.deleteScheduledTaskNameAndItsRunHistory', {
        name: selected.name,
      }),
      confirmLabel: t('schedules:schedulesPage.delete'),
    })
    if (!approved) return
    try {
      await apiJson(`/api/schedules/${encodeURIComponent(selected.id)}`, { method: 'DELETE' })
      await load()
      notify(t('schedules:schedulesPage.scheduledTaskDeleted'))
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    }
  }

  if (loading)
    return (
      <AppEmptyState>
        <RefreshCw className="animate-spin" size={23} />
        <h2>{t('schedules:schedulesPage.loadingScheduledTasks')}</h2>
      </AppEmptyState>
    )
  return (
    <>
      {error && (
        <AppError>
          <AlertTriangle size={13} />
          {error}
        </AppError>
      )}
      <div className="split-list-detail grid min-h-[100%] grid-cols-[330px_minmax(0,1fr)] gap-[12px] max-[900px]:grid-cols-[1fr] schedule-layout !grid-cols-[310px_minmax(0,1fr)] max-[900px]:[.split-list-detail&]:grid-cols-[280px_minmax(420px,1fr)] max-[900px]:[.split-list-detail&]:overflow-x-auto max-[650px]:[.split-list-detail&]:grid-cols-[1fr] max-[650px]:[.split-list-detail&]:[overflow-x:visible]">
        <Panel className="selection-list [.config-layout_>_&]:max-h-[calc(100dvh_-_280px)] [.config-layout_>_&]:overflow-y-auto max-[900px]:max-h-[300px] min-h-0 overflow-auto">
          <SectionTitle title={t('schedules:schedulesPage.taskQueue')} />
          {data.tasks.length ? (
            data.tasks.map((task) => (
              <div
                className={`schedule-list-item hover:bg-[var(--accent-soft)] [&.active]:bg-[var(--accent-soft)] [&_>_button]:grid [&_>_button]:min-w-0 [&_>_button]:min-h-[68px] [&_>_button]:grid-cols-[minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[8px] [&_>_button]:border-0 [&_>_button]:bg-transparent [&_>_button]:p-[6px_2px] [&_>_button]:text-left [&_>_button_>_span]:flex [&_>_button_>_span]:min-w-0 [&_>_button_>_span]:flex-col [&_>_button_>_span]:gap-[5px] [&_strong]:overflow-hidden [&_strong]:text-ellipsis [&_strong]:whitespace-nowrap [&_small]:overflow-hidden [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_strong]:text-[13px] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_em]:flex [&_em]:items-center [&_em]:gap-[4px] [&_em]:text-[var(--text-muted)] [&_em]:text-[13px] [&_em]:[font-style:normal] grid grid-cols-[minmax(0,1fr)_auto] items-center gap-[5px] [border-top:1px_solid_var(--stroke-soft)] rounded-[var(--r-sm)] [padding:3px_5px] ${selectedId === task.id ? 'active' : ''}`}
                key={task.id}
              >
                <button onClick={() => setSelectedId(task.id)}>
                  <span>
                    <div className="schedule-item-head [.schedule-list-item_&]:flex [.schedule-list-item_&]:min-w-0 [.schedule-list-item_&]:items-center [.schedule-list-item_&]:gap-[6px] [.schedule-list-item_&_strong]:flex-1 [.schedule-list-item_&_strong]:min-w-0">
                      <strong>{task.name}</strong>
                      <Badge tone={task.enabled ? 'green' : 'gray'}>
                        {task.enabled
                          ? t('schedules:schedulesPage.enabled')
                          : t('schedules:schedulesPage.pause')}
                      </Badge>
                    </div>
                    <small>
                      {task.targetType === 'workflow'
                        ? availableWorkflows.find((workflow) => workflow.id === task.workflowId)
                            ?.name || t('schedules:schedulesPage.workflowUnavailable')
                        : task.prompt}
                    </small>
                  </span>
                  <em>{nextRunLabel(task, language)}</em>
                </button>
              </div>
            ))
          ) : (
            <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center">
              <StarOrbit size={38} />
              <strong>{t('schedules:schedulesPage.theTimelineIsStillUnlit')}</strong>
              <span>
                {t(
                  'schedules:schedulesPage.createATaskAndLetItSetOutAutomaticallyAtTheAppointedTime',
                )}
              </span>
            </div>
          )}
        </Panel>
        {selected && draft ? (
          <div className="detail-stack flex min-w-0 flex-col gap-[12px] [.mcp-layout_>_&]:min-h-0 max-[1150px]:[.memory-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.memory-layout_>_&]:grid max-[1150px]:[.memory-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.mcp-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.mcp-layout_>_&]:grid max-[1150px]:[.mcp-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[1150px]:[.skills-layout_>_&]:[grid-column:1/-1] max-[1150px]:[.skills-layout_>_&]:grid max-[1150px]:[.skills-layout_>_&]:grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:[.memory-layout_>_&]:[grid-column:auto] max-[650px]:[.memory-layout_>_&]:grid-cols-[1fr] max-[650px]:[.mcp-layout_>_&]:[grid-column:auto] max-[650px]:[.mcp-layout_>_&]:grid-cols-[1fr] max-[650px]:[.skills-layout_>_&]:[grid-column:auto] max-[650px]:[.skills-layout_>_&]:grid-cols-[1fr]">
            <Panel>
              <AppCardHeader>
                <h2>{draft.name}</h2>
                <div className="flex items-center gap-[6px] gap-[5px]">
                  <Toggle value={draft.enabled} onChange={(enabled) => updateDraft({ enabled })} />
                  <Button
                    size="lg"
                    disabled={saving || selected.lastStatus === 'running'}
                    onClick={run}
                  >
                    {selected.lastStatus === 'running' ? (
                      <RefreshCw className="animate-spin" size={14} />
                    ) : (
                      <Play size={14} />
                    )}
                    {selected.lastStatus === 'running'
                      ? t('schedules:schedulesPage.running')
                      : t('schedules:schedulesPage.runNow')}
                  </Button>
                  <Button
                    variant="destructive"
                    size="icon"
                    title={t('schedules:schedulesPage.deleteTask')}
                    onClick={remove}
                  >
                    <Trash2 size={14} />
                  </Button>
                </div>
              </AppCardHeader>
              <ScheduleTargetFields
                targetType={draft.targetType}
                prompt={draft.prompt}
                workflowId={draft.workflowId}
                workflowInputs={draft.workflowInputs}
                workflows={availableWorkflows}
                onChange={updateDraft}
              />
              {draft.targetType === 'prompt' && (
                <ScheduleWorkspaceField
                  value={draft.cwd}
                  onChange={(cwd) => updateDraft({ cwd })}
                />
              )}
              <div className="form-grid grid gap-[9px] three [.form-grid&]:grid-cols-[repeat(3,minmax(0,1fr))] max-[650px]:[.form-grid&]:grid-cols-[1fr]">
                <FieldLabel variant="control">
                  {t('schedules:schedulesPage.frequency')}
                  <AppSelect
                    value={draft.frequency}
                    onChange={(event) =>
                      updateDraft({ frequency: event.target.value as ScheduleFrequency })
                    }
                  >
                    {Object.keys(FREQUENCIES).map((value) => (
                      <option value={value} key={value}>
                        {frequencyLabel(value as ScheduleFrequency, t)}
                      </option>
                    ))}
                  </AppSelect>
                </FieldLabel>
                {draft.frequency === 'interval' ? (
                  <FieldLabel variant="control">
                    {t('schedules:schedulesPage.runInterval')}
                    <span className="schedule-interval-input [&_input]:w-full [&_input]:min-w-0 [&_input]:h-full [&_input]:border-0 [&_input]:[outline:0] [&_input]:bg-transparent [&_input]:p-[0_9px] [&_input]:text-[var(--text)] [&_select]:h-full [&_select]:border-0 [&_select]:[border-left:1px_solid_var(--stroke)] [&_select]:[outline:0] [&_select]:bg-[var(--solid)] [&_select]:p-[0_8px] [&_select]:text-[var(--text-tertiary)] [&_select]:text-[12px] [&_[data-slot='select-trigger']]:w-auto [&_[data-slot='select-trigger']]:min-w-[74px] [&_[data-slot='select-trigger']]:[border-left:1px_solid_var(--stroke)] [&_[data-slot='select-trigger']]:rounded-[0] [&_[data-slot='select-trigger']]:bg-[var(--solid)] [&_[data-slot='select-trigger']]:text-[var(--text-tertiary)] grid h-[31px] grid-cols-[minmax(0,1fr)_auto] overflow-hidden [border:1px_solid_var(--stroke)] rounded-[var(--r-xs)] bg-[var(--surface-subtle)]">
                      <input
                        type="number"
                        min="1"
                        value={draft.intervalValue}
                        onChange={(event) =>
                          updateDraft({ intervalValue: Number(event.target.value) })
                        }
                      />
                      <AppSelect
                        value={draft.intervalUnit}
                        onChange={(event) =>
                          updateDraft({ intervalUnit: event.target.value as IntervalUnit })
                        }
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
                    <input
                      type="time"
                      value={draft.time}
                      onChange={(event) => updateDraft({ time: event.target.value })}
                    />
                  </FieldLabel>
                )}
                <FieldLabel variant="control">
                  {t('schedules:schedulesPage.timeZone')}
                  <AppSelect
                    value={draft.timezone}
                    onChange={(event) => updateDraft({ timezone: event.target.value })}
                  >
                    {TIMEZONES.map((timezone) => (
                      <option value={timezone} key={timezone}>
                        {timezone}
                      </option>
                    ))}
                  </AppSelect>
                </FieldLabel>
                {draft.targetType === 'prompt' && (
                  <ScheduleExecutionModeField
                    value={draft.executionMode}
                    onChange={(executionMode) => updateDraft({ executionMode })}
                  />
                )}
              </div>
              <div className="schedule-notification-section [&_>_div:first-child]:flex [&_>_div:first-child]:items-center [&_>_div:first-child]:justify-between [&_>_div:first-child]:gap-[8px] [&_>_div:first-child_strong]:text-[13px] [&_>_p]:mt-[4px] [&_>_p]:text-[var(--text-muted)] [&_>_p]:text-[13px] [margin-top:13px] [border-top:1px_solid_var(--stroke-soft)] [padding-top:12px]">
                <div>
                  <strong>{t('schedules:schedulesPage.notificationChannels')}</strong>
                  <button
                    type="button"
                    className="text-button hover:text-[var(--text)] border-0 bg-transparent text-[var(--text-soft)] text-[12px] font-[600] [text-decoration:underline] [text-underline-offset:2px]"
                    onClick={openNotificationSettings}
                  >
                    {t('schedules:schedulesPage.editTemplates')}
                  </button>
                </div>
                <p>{t('schedules:schedulesPage.notificationChannelsHelp')}</p>
                <div className="schedule-notification-targets [&_>_button]:grid [&_>_button]:min-h-[52px] [&_>_button]:grid-cols-[auto_minmax(0,1fr)_auto] [&_>_button]:items-center [&_>_button]:gap-[7px] [&_>_button]:[border:1px_solid_var(--stroke)] [&_>_button]:rounded-[var(--r-sm)] [&_>_button]:bg-[var(--surface-subtle)] [&_>_button]:p-[7px_8px] [&_>_button]:text-left [&_>_button_>_span]:flex [&_>_button_>_span]:min-w-0 [&_>_button_>_span]:flex-col [&_>_button_>_span]:gap-[3px] [&_strong]:text-[13px] [&_small]:overflow-hidden [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_small]:text-ellipsis [&_small]:whitespace-nowrap [&_>_button_>_svg:last-child]:text-[var(--control-muted)] [&_>_button.selected]:border-[var(--focus)] [&_>_button.selected]:bg-[var(--accent-soft)] [&_>_button.selected]:text-[var(--star-strong)] [&_>_button.selected_>_svg:last-child]:text-[var(--success)] max-[650px]:grid-cols-[1fr] grid grid-cols-[repeat(3,minmax(0,1fr))] gap-[7px] [margin-top:9px]">
                  {(
                    Object.entries(TARGETS) as Array<
                      [NotificationTarget, (typeof TARGETS)[NotificationTarget]]
                    >
                  ).map(([id, target]) => {
                    const Icon = target.Icon
                    const selected = draft.notifications.includes(id)
                    return (
                      <button
                        type="button"
                        className={selected ? 'selected' : ''}
                        onClick={() => toggleNotification(id)}
                        key={id}
                      >
                        <Icon size={15} />
                        <span>
                          <strong>{notificationTargetLabel(id, t)}</strong>
                          <small>
                            {data.notificationTargets[id].enabled
                              ? t('schedules:schedulesPage.configured')
                              : t('schedules:schedulesPage.notConfigured')}
                          </small>
                        </span>
                        <CheckCircle2 size={15} />
                      </button>
                    )
                  })}
                </div>
                <div className="flex items-center gap-[6px] [margin-top:9px]">
                  <button
                    type="button"
                    className={`schedule-notification-chip [&.selected]:border-[var(--control-selected-border)] [&.selected]:bg-[var(--control-selected-bg)] [&.selected]:text-[var(--control-selected-text)] [&.selected]:shadow-[var(--sh-1)] dark:bg-[var(--surface-subtle)] inline-flex h-[32px] items-center gap-[4px] [border:1px_solid_var(--stroke)] rounded-[var(--r-pill)] bg-[var(--surface-subtle)] text-[var(--text-muted)] [padding:0_10px] text-[13px] font-[600] ${draft.notifyOn === 'failure' ? 'selected' : ''}`}
                    onClick={() =>
                      updateDraft({ notifyOn: draft.notifyOn === 'failure' ? 'always' : 'failure' })
                    }
                  >
                    {draft.notifyOn === 'failure'
                      ? t('schedules:schedulesPage.failuresOnly')
                      : t('schedules:schedulesPage.completionAndFailure')}
                  </button>
                </div>
              </div>
              <div className="form-footer [&_>_span]:text-[var(--text-muted)] [&_>_span]:text-[13px] flex items-center justify-between gap-[10px] [margin-top:10px]">
                <span>
                  {selected.lastRunAt
                    ? t('schedules:schedulesPage.lastRunTime', {
                        time: relativeTime(selected.lastRunAt, language),
                      })
                    : t('schedules:schedulesPage.nextRunTime', {
                        time: nextRunLabel(selected, language),
                      })}
                </span>
                <Button
                  size="lg"
                  disabled={
                    saving ||
                    !scheduleTargetValid(
                      draft.targetType,
                      draft.prompt,
                      draft.workflowId,
                      draft.workflowInputs,
                      availableWorkflows,
                    )
                  }
                  onClick={save}
                >
                  {saving ? <RefreshCw className="animate-spin" size={14} /> : null}
                  {saving
                    ? t('schedules:schedulesPage.saving')
                    : t('schedules:schedulesPage.saveTask')}
                </Button>
              </div>
            </Panel>
            <Panel>
              <SectionTitle title={t('schedules:schedulesPage.recentRuns')} />
              {runs.length ? (
                runs.map((item) => (
                  <div
                    className={`schedule-run-row [&_>_svg]:text-[var(--text-muted)] [&.completed_>_svg]:text-[var(--success)] [&.failed_>_svg]:text-[var(--danger)] [&.running_>_svg]:text-[var(--star-strong)] [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_>_span]:gap-[4px] [&_strong]:[display:-webkit-box] [&_strong]:overflow-hidden [&_strong]:text-[12px] [&_strong]:leading-[1.4] [&_strong]:[-webkit-box-orient:vertical] [&_strong]:[-webkit-line-clamp:2] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_a]:text-[var(--text-soft)] [&_a]:text-[13px] [&_a]:[text-decoration:underline] [&_a]:[text-underline-offset:2px] [&_>_em]:text-[var(--text-muted)] [&_>_em]:text-[12px] [&_>_em]:[font-style:normal] [&_>_em]:whitespace-nowrap grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] [border-top:1px_solid_var(--stroke-soft)] [padding:10px_2px] ${item.status}`}
                    key={item.id}
                  >
                    {item.status === 'running' ? (
                      <RefreshCw className="animate-spin" size={15} />
                    ) : item.status === 'completed' ? (
                      <CheckCircle2 size={15} />
                    ) : (
                      <AlertTriangle size={15} />
                    )}
                    <span>
                      <strong>
                        {item.status === 'running'
                          ? t('schedules:schedulesPage.running2')
                          : item.status === 'completed'
                            ? item.summary || t('schedules:schedulesPage.taskCompleted')
                            : item.status === 'interrupted'
                              ? t('schedules:schedulesPage.taskInterrupted')
                              : item.error || t('schedules:schedulesPage.taskFailed')}
                      </strong>
                      <small>
                        {relativeTime(item.startedAt, language)} ·{' '}
                        {item.trigger === 'manual'
                          ? t('schedules:schedulesPage.manualRun')
                          : t('schedules:schedulesPage.scheduledTrigger')}
                        {item.durationMs
                          ? ` · ${t('schedules:schedulesPage.countSec', { count: Math.round(item.durationMs / 1000) })}`
                          : ''}
                      </small>
                    </span>
                    <em>
                      {new Intl.DateTimeFormat(language, {
                        hour: '2-digit',
                        minute: '2-digit',
                      }).format(new Date(item.startedAt))}
                    </em>
                  </div>
                ))
              ) : (
                <div className="channel-route-empty [&_strong]:mt-[9px] [&_strong]:text-[var(--text)] [&_strong]:text-[12px] [&_span]:mt-[4px] [&_span]:text-[13px] [&.compact]:min-h-[110px] [.workflow-assets-panel_&]:min-h-[150px] [.workflow-assets-panel_&]:border-0 [.workflow-assets-panel_&]:bg-transparent grid min-h-[185px] place-content-center justify-items-center text-[var(--text-muted)] text-center compact">
                  <StarOrbit size={32} />
                  <strong>{t('schedules:schedulesPage.noRunHistory')}</strong>
                </div>
              )}
            </Panel>
          </div>
        ) : (
          <CreateSchedulePanel
            notificationTargets={data.notificationTargets}
            workflows={availableWorkflows}
            defaultCwd={data.defaultCwd}
            onCreated={(result) => {
              setData(result.state)
              setSelectedId(result.task.id)
              notify(t('schedules:schedulesPage.scheduledTaskCreated'))
            }}
          />
        )}
      </div>
      {createOpen && (
        <CreateScheduleModal
          notificationTargets={data.notificationTargets}
          workflows={availableWorkflows}
          defaultCwd={data.defaultCwd}
          onClose={() => setCreateOpen(false)}
          onCreated={(result) => {
            setCreateOpen(false)
            setData(result.state)
            setSelectedId(result.task.id)
            notify(t('schedules:schedulesPage.scheduledTaskCreated'))
          }}
        />
      )}
    </>
  )
}

function CreateSchedulePanel({
  notificationTargets,
  workflows,
  defaultCwd,
  onCreated,
}: ScheduleCreatorProps) {
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

function CreateScheduleModal({
  notificationTargets,
  workflows,
  defaultCwd,
  onClose,
  onCreated,
}: CreateScheduleModalProps) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [targetType, setTargetType] = useState<ScheduleTargetType>('prompt')
  const [prompt, setPrompt] = useState('')
  const [workflowId, setWorkflowId] = useState('')
  const [workflowInputs, setWorkflowInputs] = useState<Record<string, unknown>>({})
  const [cwd, setCwd] = useState(defaultCwd || '')
  const [executionMode, setExecutionMode] = useState<ScheduleExecutionMode>('full-access')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
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
      setSaving(false)
    }
  }
  return (
    <div
      className="modal-backdrop max-[650px]:p-[8px] fixed z-[70] inset-0 grid place-items-center overflow-y-auto bg-[var(--modal-overlay)] [backdrop-filter:blur(3px)] [padding:20px] [overscroll-behavior:contain] [animation:fade-in_var(--d1)_var(--ease-out)]"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form
        className="modal !w-[min(430px,100%)] max-h-[calc(100dvh_-_40px)] overflow-y-auto [overscroll-behavior:contain] [border:1px_solid_var(--surface-highlight)] rounded-[var(--r-md)] bg-[var(--solid)] p-[18px] shadow-[0_26px_70px_-25px_var(--shadow-strong)] [animation:modal-in_var(--d2)_var(--ease-out)] max-[650px]:max-h-[calc(100dvh_-_16px)]"
        onSubmit={submit}
      >
        <AppCardHeader>
          <div>
            <h2>{t('schedules:schedulesPage.newScheduledTask')}</h2>
            <p>
              {t(
                'schedules:schedulesPage.youCanContinueSettingTheRunTimeAndNotificationChannelsAfterCreation',
              )}
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label={t('schedules:schedulesPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </Button>
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
    </div>
  )
}
