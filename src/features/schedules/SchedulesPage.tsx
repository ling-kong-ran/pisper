import { PageFrame } from '@/components/app/PageFrame'
import { ListDetailLayout } from '@/components/app/ListDetailLayout'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
// 定时计划页：查看/创建/启停周期任务（如每日对话/工作流运行），
// 展示下次执行时间与历史。
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Bell,
  Bot,
  Check,
  CheckCircle2,
  FolderOpen,
  MessageCircle,
  Play,
  Plus,
  Send,
  Trash2,
  TriangleAlert,
  X,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSwitch as Toggle,
  AppCardHeader,
  AppError,
} from '@/components/ui/app-primitives'
import { Badge } from '@/components/ui/badge'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { OrbitSpinner } from '@/components/ui/spinner'
import { AppSelect } from '@/components/AppSelect'
import { WorkspacePicker } from '@/components/WorkspacePicker'
import { useI18n } from '@/app/use-i18n'
import { apiJson } from '@/lib/api'
import { relativeTime } from '@/lib/format'
import { pickSystemDirectory } from '@/lib/pick-system-directory'
import { cn } from '@/lib/utils'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import type { FormEvent } from 'react'
import type { Notify } from '@/app/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

import { Button } from '@/components/ui/button'

import { FieldLabel } from '@/components/ui/field'

type NotificationTarget = 'browser' | 'feishu' | 'weixin' | 'qq' | 'telegram'
type ScheduleFrequency = 'interval' | 'daily' | 'weekly' | 'monthly'
type IntervalUnit = 'minutes' | 'hours' | 'days'
type ScheduleExecutionMode = 'full-access'
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
  qq: { name: 'QQ', Icon: MessageCircle },
  telegram: { name: 'Telegram', Icon: Send },
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
const SCHEDULE_EXECUTION_MODES: ScheduleExecutionMode[] = ['full-access']
// 表单按「目标 / 计划 / 通知」分组：组间只用一条细线隔开，不再叠卡片。
const SECTION = 'mt-4 border-t border-stroke-soft pt-1'
// 运行计划三列（频率 / 时间或间隔 / 时区），窄屏折成单列。
const SCHEDULE_GRID = 'form-grid grid gap-x-3 sm:grid-cols-3'
// 间隔 = 数值框 + 单位选择，两个独立控件并排，各自沿用字段规范的描边与焦点环。
const INTERVAL_INPUT =
  'schedule-interval-input grid grid-cols-[minmax(0,1fr)_minmax(84px,auto)] gap-1.5'
// 两段式分段控件（与历史会话页同一几何）：凹陷轨道 + 选中键帽；手机与粗指针下每段 44px。
const SEGMENT_TRACK =
  'inline-grid h-8 grid-cols-2 rounded-md border border-stroke bg-chrome p-[3px] max-[650px]:h-auto max-[650px]:w-full pointer-coarse:h-auto'
const SEGMENT =
  'inline-flex min-w-0 items-center justify-center rounded-sm border border-transparent px-3 text-sm text-content-secondary transition-colors duration-(--d-fast) hover:text-content aria-pressed:border-key-border aria-pressed:bg-key aria-pressed:font-medium aria-pressed:text-content aria-pressed:shadow-key max-[650px]:h-11 pointer-coarse:h-11'

// 运行状态方块：形状与颜色共同表达（勾 / 叉 / 警示 / 轨道），与工作流运行日志一致。
function ScheduleRunSquare({ status }: { status: ScheduleRun['status'] }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'mt-px grid size-[18px] flex-none place-items-center rounded-xs',
        status === 'completed' && 'bg-success-soft text-success',
        status === 'failed' && 'bg-danger-soft text-danger',
        status === 'interrupted' && 'bg-warning-soft text-warning-strong',
        status === 'running' && 'text-accent-fg',
      )}
    >
      {status === 'running' ? (
        <OrbitSpinner className="size-3.5" />
      ) : status === 'completed' ? (
        <Check size={12} strokeWidth={2.25} />
      ) : status === 'failed' ? (
        <X size={12} strokeWidth={2.25} />
      ) : (
        <TriangleAlert size={11} />
      )}
    </span>
  )
}

function executionModeHelp(t: ReturnType<typeof useI18n>['t']) {
  return t('schedules:schedulesPage.fullAccessHelp')
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
  if (target === 'qq') return t('schedules:schedulesPage.qq')
  if (target === 'telegram') return t('schedules:schedulesPage.telegram')
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
            {t('schedules:schedulesPage.fullAccess')}
          </option>
        ))}
      </AppSelect>
      <small>{executionModeHelp(t)}</small>
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
    <div className="grid">
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
            <div className="grid">
              <div className="schedule-workflow-inputs-heading mt-3 flex items-baseline justify-between gap-3 max-[650px]:flex-col max-[650px]:items-start max-[650px]:gap-0.5 [&_small]:text-xs [&_small]:text-content-muted [&_strong]:text-xs [&_strong]:font-medium [&_strong]:text-content-secondary">
                <strong>{t('schedules:schedulesPage.workflowInputs')}</strong>
                <small>{t('schedules:schedulesPage.workflowInputsHelp')}</small>
              </div>
              <div className="schedule-workflow-inputs grid gap-x-3 sm:grid-cols-2 [&_input[type=checkbox]]:size-4 [&_input[type=checkbox]]:justify-self-end [&_input[type=checkbox]]:accent-star">
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

  // 浏览选择工作目录：打开系统文件管理器，系统对话框无法出现时才退回手动输入。
  const browse = async () => {
    setPickerError('')
    try {
      const result = await pickSystemDirectory(
        value,
        t('common:workspacePicker.selectWorkspaceForSchedule'),
      )
      if (result.status === 'picked') onChange(result.path)
      else if (result.status === 'unavailable') setWebPickerOpen(true)
    } catch (error) {
      setPickerError(error instanceof Error ? error.message : String(error))
    }
  }

  return (
    <>
      <FieldLabel variant="control">
        {t('schedules:schedulesPage.workingDirectory')}
        <span className="schedule-workspace-input grid grid-cols-[minmax(0,1fr)_auto] gap-1.5 [&_input]:min-w-0 [&_input]:font-mono">
          <input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder={t('schedules:schedulesPage.enterTheProjectSAbsolutePath')}
          />
          {/* 浏览键与输入框等高（网格行拉伸），作为次级键帽而不是第二个主操作。 */}
          <Button
            type="button"
            variant="secondary"
            className="h-auto min-h-8.5"
            onClick={() => void browse()}
          >
            <FolderOpen />
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
  const [detailOpen, setDetailOpen] = useState(false)

  const { t, language } = useI18n()
  const [data, setData] = useState<SchedulesData>({
    tasks: [],
    runs: [],
    notificationTargets: {
      browser: { enabled: false },
      feishu: { enabled: false },
      weixin: { enabled: false },
      qq: { enabled: false },
      telegram: { enabled: false },
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
    return <PageState kind="loading" title={t('schedules:schedulesPage.loadingScheduledTasks')} />
  return (
    <PageFrame variant="list-detail">
      <div className="flex h-full min-h-0 flex-col gap-3">
        {error && (
          <AppError className="mx-3 mb-0">
            <AlertTriangle size={13} />
            {error}
          </AppError>
        )}
        <ListDetailLayout
          selected={detailOpen}
          onBack={() => setDetailOpen(false)}
          backLabel={t('common:pageTemplate.backToList')}
          list={
            <section className="min-w-0">
              {/* 队列标题 12/500 弱化文字 + 等宽计数，与历史会话的分组标题同一节奏。 */}
              <h2 className="flex h-[30px] items-end justify-between px-1 pb-1.5 text-xs font-medium tracking-[.02em] text-content-muted">
                <span>{t('schedules:schedulesPage.taskQueue')}</span>
                <span className="font-normal tabular-nums">{data.tasks.length}</span>
              </h2>
              {data.tasks.length ? (
                <ul className="flex flex-col gap-0.5">
                  {data.tasks.map((task) => {
                    const active = selectedId === task.id
                    return (
                      <li
                        className={cn(
                          'schedule-list-item relative rounded-md transition-colors duration-(--d-fast) hover:bg-surface-subtle',
                          active && 'active bg-surface-muted hover:bg-surface-muted',
                        )}
                        key={task.id}
                      >
                        {/* 选中行：次级表面 + 3px 强调色刻度，两种主题下都不只靠底色区分。 */}
                        {active && (
                          <i
                            aria-hidden="true"
                            className="absolute inset-y-2.5 left-0 w-[3px] rounded-r-[3px] bg-star"
                          />
                        )}
                        <button
                          type="button"
                          aria-current={active || undefined}
                          className="grid min-h-[60px] w-full min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 py-2 pr-2.5 pl-3 text-left focus-visible:-outline-offset-2"
                          onClick={() => {
                            setDetailOpen(true)
                            setSelectedId(task.id)
                          }}
                        >
                          <strong className="truncate text-sm font-medium text-content max-[650px]:text-[15px]">
                            {task.name}
                          </strong>
                          <em className="text-xs text-content-muted not-italic tabular-nums">
                            {nextRunLabel(task, language)}
                          </em>
                          <span className="col-span-2 flex min-w-0 items-center gap-2">
                            {task.lastStatus === 'running' ? (
                              <Badge variant="run" className="gap-1.5">
                                <OrbitSpinner />
                                {t('schedules:schedulesPage.running')}
                              </Badge>
                            ) : (
                              <Badge variant={task.enabled ? 'ok' : 'default'}>
                                {task.enabled
                                  ? t('schedules:schedulesPage.enabled')
                                  : t('schedules:schedulesPage.pause')}
                              </Badge>
                            )}
                            {task.lastStatus === 'failed' && (
                              <Badge variant="bad">
                                {t('schedules:schedulesPage.lastRunFailed')}
                              </Badge>
                            )}
                            <small
                              className={cn(
                                'min-w-0 truncate text-xs text-content-secondary',
                                task.targetType === 'workflow' && 'font-mono',
                              )}
                            >
                              {task.targetType === 'workflow'
                                ? availableWorkflows.find(
                                    (workflow) => workflow.id === task.workflowId,
                                  )?.name || t('schedules:schedulesPage.workflowUnavailable')
                                : task.prompt}
                            </small>
                          </span>
                        </button>
                      </li>
                    )
                  })}
                </ul>
              ) : (
                <InlineEmptyState
                  title={t('schedules:schedulesPage.theTimelineIsStillUnlit')}
                  description={t(
                    'schedules:schedulesPage.createATaskAndLetItSetOutAutomaticallyAtTheAppointedTime',
                  )}
                />
              )}
            </section>
          }
          detail={
            selected && draft ? (
              <div className="flex min-w-0 flex-col gap-4">
                {/* 手机上详情已独占整屏，卡片外框退为无边平铺，避免与页面边距叠成双层框。 */}
                <Panel className="p-4 py-4 max-[650px]:border-0 max-[650px]:bg-transparent max-[650px]:p-0">
                  <AppCardHeader className="items-center max-[650px]:flex-wrap max-[650px]:items-start">
                    <h2 className="min-w-0 text-[15px]! font-medium break-words">{draft.name}</h2>
                    <div className="flex min-w-0 flex-wrap items-center gap-2 max-[650px]:w-full">
                      <Toggle
                        value={draft.enabled}
                        onChange={(enabled) => updateDraft({ enabled })}
                        ariaLabel={draft.name}
                      />
                      <Button
                        variant="secondary"
                        className="max-[650px]:h-11"
                        disabled={saving || selected.lastStatus === 'running'}
                        onClick={run}
                      >
                        {selected.lastStatus === 'running' ? <OrbitSpinner /> : <Play />}
                        {selected.lastStatus === 'running'
                          ? t('schedules:schedulesPage.running')
                          : t('schedules:schedulesPage.runNow')}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="hover:bg-danger-soft hover:text-danger max-[650px]:ml-auto max-[650px]:size-11"
                        title={t('schedules:schedulesPage.deleteTask')}
                        aria-label={t('schedules:schedulesPage.deleteTask')}
                        onClick={remove}
                      >
                        <Trash2 />
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
                  <div className={SECTION}>
                    <div className={SCHEDULE_GRID}>
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
                          <span className={INTERVAL_INPUT}>
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
                            className="tabular-nums"
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
                  </div>
                  <div className={cn('schedule-notification-section pt-3', SECTION)}>
                    <div className="flex items-center justify-between gap-2">
                      <strong className="text-sm font-medium">
                        {t('schedules:schedulesPage.notificationChannels')}
                      </strong>
                      <button
                        type="button"
                        className="text-button text-xs font-medium text-content-secondary underline underline-offset-2 hover:text-content max-[650px]:min-h-11"
                        onClick={openNotificationSettings}
                      >
                        {t('schedules:schedulesPage.editTemplates')}
                      </button>
                    </div>
                    <p className="mt-0.5 text-xs text-content-muted">
                      {t('schedules:schedulesPage.notificationChannelsHelp')}
                    </p>
                    {/* 渠道开关是可按下的磁贴：开启 = 强调色软底 + 强调描边 + 勾，关闭只留细线。 */}
                    <div className="schedule-notification-targets mt-2.5 grid grid-cols-[repeat(auto-fill,minmax(132px,1fr))] gap-2">
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
                            aria-pressed={selected}
                            className={cn(
                              'grid min-h-[52px] grid-cols-[16px_minmax(0,1fr)_16px] items-center gap-2 rounded-md border px-2.5 py-2 text-left transition-colors duration-(--d-fast)',
                              selected
                                ? 'selected border-star-border bg-star-soft text-star-strong'
                                : 'border-stroke bg-panel text-content hover:bg-surface-subtle',
                            )}
                            onClick={() => toggleNotification(id)}
                            key={id}
                          >
                            <Icon className="size-4" />
                            <span className="flex min-w-0 flex-col gap-0.5">
                              <strong className="truncate text-sm font-medium">
                                {notificationTargetLabel(id, t)}
                              </strong>
                              <small className="truncate text-xs text-content-muted">
                                {data.notificationTargets[id].enabled
                                  ? t('schedules:schedulesPage.configured')
                                  : t('schedules:schedulesPage.notConfigured')}
                              </small>
                            </span>
                            <CheckCircle2
                              className={cn(
                                'size-4',
                                selected ? 'text-accent-fg' : 'text-content-faint',
                              )}
                            />
                          </button>
                        )
                      })}
                    </div>
                    <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1.5">
                      <span id="schedule-notify-on" className="text-xs text-content-muted">
                        {t('schedules:schedulesPage.notifyWhen')}
                      </span>
                      <div
                        role="group"
                        aria-labelledby="schedule-notify-on"
                        className={cn('schedule-notification-chip', SEGMENT_TRACK)}
                      >
                        {(['always', 'failure'] as const).map((value) => (
                          <button
                            type="button"
                            key={value}
                            aria-pressed={draft.notifyOn === value}
                            className={SEGMENT}
                            onClick={() => updateDraft({ notifyOn: value })}
                          >
                            {value === 'failure'
                              ? t('schedules:schedulesPage.failuresOnly')
                              : t('schedules:schedulesPage.completionAndFailure')}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>
                  <div className="form-footer mt-4 flex items-center justify-between gap-2.5 border-t border-stroke-soft pt-3 max-[650px]:flex-wrap">
                    <span className="text-xs text-content-muted tabular-nums">
                      {selected.lastRunAt
                        ? t('schedules:schedulesPage.lastRunTime', {
                            time: relativeTime(selected.lastRunAt, language),
                          })
                        : t('schedules:schedulesPage.nextRunTime', {
                            time: nextRunLabel(selected, language),
                          })}
                    </span>
                    <Button
                      className="max-[650px]:h-11 max-[650px]:w-full"
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
                      {saving ? <OrbitSpinner className="text-current" /> : null}
                      {saving
                        ? t('schedules:schedulesPage.saving')
                        : t('schedules:schedulesPage.saveTask')}
                    </Button>
                  </div>
                </Panel>
                {/* 最近执行：与工具活动日志同一语言的细线卡片，状态方块 + 摘要 + 等宽时间。 */}
                <section className="overflow-hidden rounded-md border border-stroke bg-panel">
                  <h3 className="flex min-h-10 items-center gap-2 border-b border-stroke-soft bg-surface-subtle px-3.5 text-sm font-medium">
                    {t('schedules:schedulesPage.recentRuns')}
                    <span className="font-normal text-content-muted tabular-nums">
                      {runs.length}
                    </span>
                  </h3>
                  {runs.length ? (
                    <ul className="divide-y divide-stroke-soft">
                      {runs.map((item) => (
                        <li
                          className={`schedule-run-row grid min-h-11 grid-cols-[18px_minmax(0,1fr)_auto] items-start gap-2.5 px-3.5 py-2.5 ${item.status}`}
                          key={item.id}
                        >
                          <ScheduleRunSquare status={item.status} />
                          <span className="flex min-w-0 flex-col gap-0.5">
                            <strong
                              className={cn(
                                'line-clamp-2 text-sm leading-5 font-medium',
                                item.status === 'failed' ? 'text-danger' : 'text-content',
                              )}
                            >
                              {item.status === 'running'
                                ? t('schedules:schedulesPage.running2')
                                : item.status === 'completed'
                                  ? item.summary || t('schedules:schedulesPage.taskCompleted')
                                  : item.status === 'interrupted'
                                    ? t('schedules:schedulesPage.taskInterrupted')
                                    : item.error || t('schedules:schedulesPage.taskFailed')}
                            </strong>
                            <small className="text-xs text-content-muted tabular-nums">
                              {relativeTime(item.startedAt, language)} ·{' '}
                              {item.trigger === 'manual'
                                ? t('schedules:schedulesPage.manualRun')
                                : t('schedules:schedulesPage.scheduledTrigger')}
                              {item.durationMs
                                ? ` · ${t('schedules:schedulesPage.countSec', { count: Math.round(item.durationMs / 1000) })}`
                                : ''}
                            </small>
                          </span>
                          <em className="font-mono text-xs leading-5 text-content-secondary not-italic tabular-nums">
                            {new Intl.DateTimeFormat(language, {
                              hour: '2-digit',
                              minute: '2-digit',
                            }).format(new Date(item.startedAt))}
                          </em>
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <InlineEmptyState compact title={t('schedules:schedulesPage.noRunHistory')} />
                  )}
                </section>
              </div>
            ) : (
              <CreateSchedulePanel
                notificationTargets={data.notificationTargets}
                workflows={availableWorkflows}
                defaultCwd={data.defaultCwd}
                onCreated={(result) => {
                  setData(result.state)
                  setSelectedId(result.task.id)
                  setDetailOpen(true)
                  notify(t('schedules:schedulesPage.scheduledTaskCreated'))
                }}
              />
            )
          }
        />
        {/* 每次打开都重新挂载，保证新建表单从空白开始。 */}
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
              setDetailOpen(true)
              notify(t('schedules:schedulesPage.scheduledTaskCreated'))
            }}
          />
        )}
      </div>
    </PageFrame>
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
    <Panel className="p-4 py-4">
      <AppCardHeader>
        <div>
          <h2 className="text-[15px]! font-medium">
            {t('schedules:schedulesPage.newScheduledTask')}
          </h2>
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
      <div className={cn(SECTION, SCHEDULE_GRID)}>
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
            <span className={INTERVAL_INPUT}>
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
            <input
              type="time"
              className="tabular-nums"
              value={time}
              onChange={(event) => setTime(event.target.value)}
            />
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
      <div className="form-footer mt-4 flex items-center justify-between gap-2.5 border-t border-stroke-soft pt-3 max-[650px]:flex-wrap">
        <span className="text-xs text-content-muted">
          {t('schedules:schedulesPage.enabledAutomaticallyAfterCreation')}
        </span>
        <Button
          className="max-[650px]:h-11 max-[650px]:w-full"
          disabled={
            saving ||
            !name.trim() ||
            !scheduleTargetValid(targetType, prompt, workflowId, workflowInputs, workflows)
          }
          onClick={create}
        >
          {saving ? <OrbitSpinner className="text-current" /> : <Plus />}
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
  // 新建对话框沿用统一 Dialog：实心浮层、无模糊遮罩、焦点受困与 Esc 关闭由原语负责。
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent size="compact">
        <form className="grid min-w-0" onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>{t('schedules:schedulesPage.newScheduledTask')}</DialogTitle>
            <DialogDescription>
              {t(
                'schedules:schedulesPage.youCanContinueSettingTheRunTimeAndNotificationChannelsAfterCreation',
              )}
            </DialogDescription>
          </DialogHeader>
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
          <div className="mt-5 flex justify-end gap-2 max-[650px]:[&>button]:h-11 max-[650px]:[&>button]:flex-1">
            <Button type="button" variant="secondary" onClick={onClose}>
              {t('schedules:schedulesPage.cancel')}
            </Button>
            <Button
              type="submit"
              disabled={
                saving ||
                !name.trim() ||
                !scheduleTargetValid(targetType, prompt, workflowId, workflowInputs, workflows)
              }
            >
              {saving ? <OrbitSpinner className="text-current" /> : <Plus />}
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
