import { FEATURE_SEARCH_INPUT_ID } from '@/app/more-pages'
import {
  FeatureAlert,
  FeatureList,
  FeaturePage,
  FeatureRow,
  FeatureSearch,
  FeatureSection,
  FeatureToolbar,
} from '@/components/app/FeaturePage'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { StatusGlyph } from '@/components/app/StatusGlyph'
// 定时任务页（功能页模板，规格 v2 第 10 节 A6 第 3 批）：每个任务一行（名称、执行目标、频率、下次运行），
// 展开在行下方编辑（目标、频率与时区、通知渠道）并查看最近执行；新建在工具行（对话框），
// 还没有任务时直接给出完整的新建表单。
import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Bell,
  Bot,
  CalendarClock,
  CheckCircle2,
  FolderOpen,
  MessageCircle,
  Play,
  Plus,
  Send,
  Trash2,
} from 'lucide-react'
import {
  AppCard as Panel,
  AppSwitch as Toggle,
  AppCardHeader,
  AppError,
} from '@/components/ui/app-primitives'
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
import { useShortcutLabel } from '@/lib/shortcuts'
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
  // “‹ 功能”：从功能进入时由应用壳给出（规格 2.4）。
  onBack?: () => void
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

// 运行状态位：状态字形（勾 / 细 ✕ / 弧），失败与中断都是中性的已发生结果（规格 6.1、6.5），不用红绿。
function ScheduleRunSquare({ status }: { status: ScheduleRun['status'] }) {
  return (
    <span aria-hidden="true" className="mt-px grid size-[18px] flex-none place-items-center">
      <StatusGlyph
        status={status === 'running' ? 'run' : status === 'completed' ? 'done' : 'failed'}
        className="size-3.5"
      />
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
  onBack,
  openNotificationSettings,
  requestConfirm,
}: SchedulesPageProps) {
  const { t, language } = useI18n()
  const searchShortcut = useShortcutLabel('focusSearch')
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
  // 展开的任务就是正在编辑的任务（同一时刻只展开一行）。草稿按任务 id 保存在页面驻留期间：
  // 收起、换展开另一行再回来，未保存的修改都还在（原列表 / 详情两栏切换时详情常驻，编辑不丢）；
  // 没改过的任务不存草稿，直接用服务端副本，轮询刷新后读数也跟着更新。保存成功或任务被删除时丢弃该任务的草稿。
  const [selectedId, setSelectedId] = useState('')
  const [drafts, setDrafts] = useState<Record<string, ScheduleDraft>>({})
  const [loading, setLoading] = useState(true)
  // 至少读成功过一次：首次读取失败显示可重试的错误，不显示成“时间轴尚未点亮”。
  const [loaded, setLoaded] = useState(false)
  const [loadError, setLoadError] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [createOpen, setCreateOpen] = useState(false)
  // 搜索词由页面自己持有（换页即清空，与原共享页头一致）。
  const [query, setQuery] = useState('')

  // 加载计划任务与运行记录；展开的任务已被删除时收起。
  const load = useCallback(async () => {
    try {
      const result = await apiJson<SchedulesData>('/api/schedules')
      setData(result)
      setLoaded(true)
      setLoadError('')
      setSelectedId((current) => (result.tasks.some((task) => task.id === current) ? current : ''))
      setDrafts((current) => {
        const kept = Object.entries(current).filter(([id]) =>
          result.tasks.some((task) => task.id === id),
        )
        return kept.length === Object.keys(current).length ? current : Object.fromEntries(kept)
      })
    } catch (caught) {
      setLoadError(caught instanceof Error ? caught.message : String(caught))
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
  const draft = selected ? (drafts[selected.id] ?? taskDraft(selected)) : null
  const runs = useMemo(
    () =>
      data.runs
        .filter((run) => run.taskId === selectedId)
        .sort((a, b) => new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime())
        .slice(0, 20),
    [data.runs, selectedId],
  )
  const workflowName = (task: ScheduleTask) =>
    availableWorkflows.find((workflow) => workflow.id === task.workflowId)?.name ||
    t('schedules:schedulesPage.workflowUnavailable')
  const needle = query.trim().toLocaleLowerCase()
  const visibleTasks = needle
    ? data.tasks.filter((task) =>
        `${task.name} ${task.targetType === 'workflow' ? workflowName(task) : task.prompt}`
          .toLocaleLowerCase()
          .includes(needle),
      )
    : data.tasks
  const enabledCount = data.tasks.filter((task) => task.enabled).length
  const updateDraft = (patch: Partial<ScheduleDraft>) => {
    if (!selected) return
    const base = taskDraft(selected)
    setDrafts((current) => ({
      ...current,
      [base.id]: { ...(current[base.id] ?? base), ...patch },
    }))
  }
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
      setDrafts((current) =>
        Object.fromEntries(Object.entries(current).filter(([id]) => id !== result.task.id)),
      )
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

  // 删除任务（确认后），成功后收起。
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

  const created = (result: ScheduleMutationResult) => {
    setData(result.state)
    setSelectedId(result.task.id)
    notify(t('schedules:schedulesPage.scheduledTaskCreated'))
  }

  // 行右侧的状态：运行中是中性弧；暂停、下次运行时间是中性文字；上次失败是细 ✕ + 文字（已发生的结果，
  // 不是等人处理的事，规格 6.1 不用红色也不用琥珀）。
  const taskStatus = (task: ScheduleTask) => (
    <span className="flex min-w-0 flex-col items-end gap-0.5 max-[650px]:flex-row max-[650px]:items-center max-[650px]:gap-2">
      {task.lastStatus === 'running' ? (
        <span className="inline-flex items-center gap-1 text-content-secondary">
          <StatusGlyph status="run" className="size-3.5" />
          {t('schedules:schedulesPage.running')}
        </span>
      ) : (
        <span className="whitespace-nowrap">
          {task.enabled
            ? t('schedules:schedulesPage.nextRunTime', { time: nextRunLabel(task, language) })
            : t('schedules:schedulesPage.pause')}
        </span>
      )}
      {task.lastStatus === 'failed' && (
        <span className="inline-flex items-center gap-1 text-content-secondary">
          <StatusGlyph status="failed" className="size-3.5" />
          {t('schedules:schedulesPage.lastRunFailed')}
        </span>
      )}
    </span>
  )

  const title = t('navigation:navigation.schedules')
  return (
    <FeaturePage
      title={title}
      description={t('schedules:schedulesPage.lede')}
      back={
        onBack && {
          label: t('navigation:sidebar.features'),
          ariaLabel: t('navigation:shell.backToFeaturesLabel'),
          onBack,
        }
      }
      toolbar={
        <FeatureToolbar
          search={
            <FeatureSearch
              id={FEATURE_SEARCH_INPUT_ID}
              value={query}
              onValueChange={setQuery}
              placeholder={t('navigation:pageHeader.searchPage', { page: title })}
              shortcut={searchShortcut}
              clearLabel={t('navigation:more.clearSearch')}
            />
          }
          actions={
            <Button
              type="button"
              className="h-9"
              disabled={!loaded}
              onClick={() => setCreateOpen(true)}
            >
              <Plus />
              {t('schedules:schedulesPage.newTask')}
            </Button>
          }
        />
      }
    >
      {loading ? (
        <PageState kind="loading" title={t('schedules:schedulesPage.loadingScheduledTasks')} />
      ) : loadError && !loaded ? (
        <PageState
          kind="error"
          title={t('schedules:schedulesPage.loadFailed')}
          description={loadError}
          action={
            <Button
              variant="secondary"
              className="max-[650px]:h-11"
              onClick={() => {
                setLoading(true)
                void load()
              }}
            >
              {t('common:pageTemplate.retry')}
            </Button>
          }
        />
      ) : (
        <>
          {/* 读到之后的操作失败（保存、运行、删除）与刷新失败：写在列表上方，列表保留。 */}
          {(error || loadError) && <FeatureAlert>{error || loadError}</FeatureAlert>}
          <FeatureSection
            title={t('schedules:schedulesPage.tasks')}
            count={data.tasks.length}
            meta={
              data.tasks.length
                ? t('schedules:schedulesPage.enabledCount', { count: enabledCount })
                : undefined
            }
          >
            {visibleTasks.length ? (
              <FeatureList>
                {visibleTasks.map((task) => {
                  const expanded = selectedId === task.id
                  return (
                    <FeatureRow
                      key={task.id}
                      data-schedule-task={task.id}
                      className="schedule-list-item"
                      icon={<CalendarClock />}
                      name={task.name}
                      description={
                        task.targetType === 'workflow' ? (
                          <span className="font-mono">{workflowName(task)}</span>
                        ) : (
                          task.prompt
                        )
                      }
                      meta={
                        <>
                          <span>{frequencyLabel(task.frequency, t)}</span>
                          <span aria-hidden="true">·</span>
                          <span className="tabular-nums">
                            {task.frequency === 'interval'
                              ? `${task.intervalValue} ${intervalUnitLabel(task.intervalUnit, t)}`
                              : task.time}
                          </span>
                          <span aria-hidden="true">·</span>
                          <span className="truncate">{task.timezone}</span>
                        </>
                      }
                      status={taskStatus(task)}
                      expanded={expanded}
                      onExpandedChange={(open) => setSelectedId(open ? task.id : '')}
                      toggleLabel={t('schedules:schedulesPage.toggleTaskDetails', {
                        name: task.name,
                      })}
                      detail={
                        expanded && draft && selected ? (
                          <ScheduleTaskEditor
                            task={selected}
                            draft={draft}
                            runs={runs}
                            saving={saving}
                            workflows={availableWorkflows}
                            notificationTargets={data.notificationTargets}
                            language={language}
                            onUpdate={updateDraft}
                            onToggleNotification={toggleNotification}
                            onRun={run}
                            onRemove={remove}
                            onSave={save}
                            onEditTemplates={openNotificationSettings}
                          />
                        ) : null
                      }
                    />
                  )
                })}
              </FeatureList>
            ) : (
              <InlineEmptyState
                title={
                  data.tasks.length
                    ? t('schedules:schedulesPage.noMatchingTasks')
                    : t('schedules:schedulesPage.theTimelineIsStillUnlit')
                }
                description={
                  data.tasks.length
                    ? undefined
                    : t(
                        'schedules:schedulesPage.createATaskAndLetItSetOutAutomaticallyAtTheAppointedTime',
                      )
                }
              />
            )}
          </FeatureSection>
          {/* 还没有任务时直接给出完整的新建表单（含频率、时间与时区），与原来详情区的空态一致。 */}
          {!data.tasks.length && (
            <div className="mt-7" data-schedule-create>
              <CreateSchedulePanel
                notificationTargets={data.notificationTargets}
                workflows={availableWorkflows}
                defaultCwd={data.defaultCwd}
                onCreated={created}
              />
            </div>
          )}
        </>
      )}
      {/* 每次打开都重新挂载，保证新建表单从空白开始。 */}
      {createOpen && (
        <CreateScheduleModal
          notificationTargets={data.notificationTargets}
          workflows={availableWorkflows}
          defaultCwd={data.defaultCwd}
          onClose={() => setCreateOpen(false)}
          onCreated={(result) => {
            setCreateOpen(false)
            created(result)
          }}
        />
      )}
    </FeaturePage>
  )
}

// 展开的任务：原详情区的编辑表单与最近执行，放在行下方（不再是右侧详情栏）。
function ScheduleTaskEditor({
  task,
  draft,
  runs,
  saving,
  workflows,
  notificationTargets,
  language,
  onUpdate,
  onToggleNotification,
  onRun,
  onRemove,
  onSave,
  onEditTemplates,
}: {
  task: ScheduleTask
  draft: ScheduleDraft
  runs: ScheduleRun[]
  saving: boolean
  workflows: ScheduleWorkflow[]
  notificationTargets: NotificationTargets
  language: string
  onUpdate: (patch: Partial<ScheduleDraft>) => void
  onToggleNotification: (target: NotificationTarget) => void
  onRun: () => void
  onRemove: () => void
  onSave: () => void
  onEditTemplates: () => void
}) {
  const { t } = useI18n()
  const running = task.lastStatus === 'running'
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <div className="min-w-0">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <label className="mr-auto inline-flex min-h-8 items-center gap-2.5 text-sm text-content max-[650px]:min-h-11">
            <Toggle
              value={draft.enabled}
              onChange={(enabled) => onUpdate({ enabled })}
              ariaLabel={t('schedules:schedulesPage.enableTaskName', { name: draft.name })}
            />
            {draft.enabled
              ? t('schedules:schedulesPage.enabled')
              : t('schedules:schedulesPage.pause')}
          </label>
          <Button
            variant="secondary"
            size="sm"
            className="max-[650px]:h-11"
            disabled={saving || running}
            onClick={onRun}
          >
            {running ? <OrbitSpinner /> : <Play />}
            {running ? t('schedules:schedulesPage.running') : t('schedules:schedulesPage.runNow')}
          </Button>
          <Button
            variant="ghost"
            size="icon-sm"
            className="hover:bg-danger-soft hover:text-danger max-[650px]:size-11"
            title={t('schedules:schedulesPage.deleteTask')}
            aria-label={t('schedules:schedulesPage.deleteTask')}
            onClick={onRemove}
          >
            <Trash2 />
          </Button>
        </div>
        <ScheduleTargetFields
          targetType={draft.targetType}
          prompt={draft.prompt}
          workflowId={draft.workflowId}
          workflowInputs={draft.workflowInputs}
          workflows={workflows}
          onChange={onUpdate}
        />
        {draft.targetType === 'prompt' && (
          <ScheduleWorkspaceField value={draft.cwd} onChange={(cwd) => onUpdate({ cwd })} />
        )}
        <div className={SECTION}>
          <div className={SCHEDULE_GRID}>
            <FieldLabel variant="control">
              {t('schedules:schedulesPage.frequency')}
              <AppSelect
                value={draft.frequency}
                onChange={(event) =>
                  onUpdate({ frequency: event.target.value as ScheduleFrequency })
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
                    onChange={(event) => onUpdate({ intervalValue: Number(event.target.value) })}
                  />
                  <AppSelect
                    value={draft.intervalUnit}
                    onChange={(event) =>
                      onUpdate({ intervalUnit: event.target.value as IntervalUnit })
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
                  onChange={(event) => onUpdate({ time: event.target.value })}
                />
              </FieldLabel>
            )}
            <FieldLabel variant="control">
              {t('schedules:schedulesPage.timeZone')}
              <AppSelect
                value={draft.timezone}
                onChange={(event) => onUpdate({ timezone: event.target.value })}
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
                onChange={(executionMode) => onUpdate({ executionMode })}
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
              onClick={onEditTemplates}
            >
              {t('schedules:schedulesPage.editTemplates')}
            </button>
          </div>
          <p className="mt-0.5 text-xs text-content-muted">
            {t('schedules:schedulesPage.notificationChannelsHelp')}
          </p>
          {/* 渠道开关是可按下的磁贴：开启 = 石墨软底 + 描边 + 勾，关闭只留细线。 */}
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
                  onClick={() => onToggleNotification(id)}
                  key={id}
                >
                  <Icon className="size-4" />
                  <span className="flex min-w-0 flex-col gap-0.5">
                    <strong className="truncate text-sm font-medium">
                      {notificationTargetLabel(id, t)}
                    </strong>
                    <small className="truncate text-xs text-content-muted">
                      {notificationTargets[id].enabled
                        ? t('schedules:schedulesPage.configured')
                        : t('schedules:schedulesPage.notConfigured')}
                    </small>
                  </span>
                  <CheckCircle2
                    className={cn('size-4', selected ? 'text-star-strong' : 'text-content-faint')}
                  />
                </button>
              )
            })}
          </div>
          <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1.5">
            <span id={`schedule-notify-on-${task.id}`} className="text-xs text-content-muted">
              {t('schedules:schedulesPage.notifyWhen')}
            </span>
            <div
              role="group"
              aria-labelledby={`schedule-notify-on-${task.id}`}
              className={cn('schedule-notification-chip', SEGMENT_TRACK)}
            >
              {(['always', 'failure'] as const).map((value) => (
                <button
                  type="button"
                  key={value}
                  aria-pressed={draft.notifyOn === value}
                  className={SEGMENT}
                  onClick={() => onUpdate({ notifyOn: value })}
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
            {task.lastRunAt
              ? t('schedules:schedulesPage.lastRunTime', {
                  time: relativeTime(task.lastRunAt, language),
                })
              : t('schedules:schedulesPage.nextRunTime', {
                  time: nextRunLabel(task, language),
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
                workflows,
              )
            }
            onClick={onSave}
          >
            {saving ? <OrbitSpinner className="text-current" /> : null}
            {saving ? t('schedules:schedulesPage.saving') : t('schedules:schedulesPage.saveTask')}
          </Button>
        </div>
      </div>
      {/* 最近执行：状态字形 + 摘要 + 等宽时间；失败与中断只靠形状区分，不用红色。 */}
      <section aria-labelledby={`schedule-runs-${task.id}`} className="min-w-0">
        <h3
          id={`schedule-runs-${task.id}`}
          className="mb-1.5 text-[13px] font-medium text-content-muted tabular-nums"
        >
          {t('schedules:schedulesPage.recentRuns')} · {runs.length}
        </h3>
        {runs.length ? (
          <ul className="divide-y divide-stroke-soft overflow-hidden rounded-[var(--r-md)] border border-stroke">
            {runs.map((item) => (
              <li
                className={`schedule-run-row grid min-h-11 grid-cols-[18px_minmax(0,1fr)_auto] items-start gap-2.5 px-3 py-2.5 ${item.status}`}
                key={item.id}
              >
                <ScheduleRunSquare status={item.status} />
                <span className="flex min-w-0 flex-col gap-0.5">
                  <strong className="line-clamp-2 text-sm leading-5 font-medium text-content">
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
