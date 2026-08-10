import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Bell,
  Bot,
  CheckCircle2,
  ChevronDown,
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

type NotificationTarget = 'browser' | 'feishu' | 'weixin'
type ScheduleFrequency = 'interval' | 'daily' | 'weekly' | 'monthly'
type IntervalUnit = 'minutes' | 'hours' | 'days'
type ScheduleExecutionMode = 'full-access' | 'read-only'
type ScheduleStatus = 'idle' | 'running' | 'completed' | 'failed' | 'interrupted'
type ScheduleTask = {
  id: string
  name: string
  prompt: string
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
  | 'prompt'
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
}
type NotificationTargets = Record<NotificationTarget, { enabled: boolean }>
type SchedulesData = {
  tasks: ScheduleTask[]
  runs: ScheduleRun[]
  notificationTargets: NotificationTargets
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
    <label className="field-label">
      {t('schedules:schedulesPage.executionMode')}
      <span className="select-wrap">
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
        <ChevronDown size={13} />
      </span>
      <small>{executionModeHelp(value, t)}</small>
    </label>
  )
}

function ScheduleWorkspaceField({ value, onChange }: ScheduleWorkspaceFieldProps) {
  const { t } = useI18n()
  const [pickerError, setPickerError] = useState('')
  const [webPickerOpen, setWebPickerOpen] = useState(false)

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
      <label className="field-label">
        {t('schedules:schedulesPage.workingDirectory')}
        <span className="schedule-workspace-input">
          <input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder={t('schedules:schedulesPage.enterTheProjectSAbsolutePath')}
          />
          <button type="button" className="button secondary" onClick={() => void browse()}>
            <FolderOpen size={13} />
            {t('schedules:schedulesPage.browseDirectories')}
          </button>
        </span>
        <small>{t('schedules:schedulesPage.theScheduledAgentWillRunInThisDirectory')}</small>
      </label>
      {pickerError && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {pickerError}
        </div>
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
    prompt: task.prompt,
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
    defaultCwd: '',
  })
  const [selectedId, setSelectedId] = useState('')
  const [draft, setDraft] = useState<ScheduleDraft | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [createOpen, setCreateOpen] = useState(false)
  usePagePrimaryAction(registerPrimaryAction, () => setCreateOpen(true))

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
      <Panel className="empty-state">
        <RefreshCw className="spin" size={23} />
        <h2>{t('schedules:schedulesPage.loadingScheduledTasks')}</h2>
      </Panel>
    )
  return (
    <>
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      <div className="split-list-detail schedule-layout">
        <Panel className="selection-list">
          <SectionTitle title={t('schedules:schedulesPage.taskQueue')} />
          {data.tasks.length ? (
            data.tasks.map((task) => (
              <div
                className={`schedule-list-item ${selectedId === task.id ? 'active' : ''}`}
                key={task.id}
              >
                <button onClick={() => setSelectedId(task.id)}>
                  <span>
                    <div className="schedule-item-head">
                      <strong>{task.name}</strong>
                      <Badge tone={task.enabled ? 'green' : 'gray'}>
                        {task.enabled
                          ? t('schedules:schedulesPage.enabled')
                          : t('schedules:schedulesPage.pause')}
                      </Badge>
                    </div>
                    <small>{task.prompt}</small>
                  </span>
                  <em>{nextRunLabel(task, language)}</em>
                </button>
              </div>
            ))
          ) : (
            <div className="channel-route-empty">
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
          <div className="detail-stack">
            <Panel>
              <div className="card-head">
                <h2>{draft.name}</h2>
                <div className="schedule-head-actions">
                  <Toggle value={draft.enabled} onChange={(enabled) => updateDraft({ enabled })} />
                  <button
                    className="button dark"
                    disabled={saving || selected.lastStatus === 'running'}
                    onClick={run}
                  >
                    {selected.lastStatus === 'running' ? (
                      <RefreshCw className="spin" size={14} />
                    ) : (
                      <Play size={14} />
                    )}
                    {selected.lastStatus === 'running'
                      ? t('schedules:schedulesPage.running')
                      : t('schedules:schedulesPage.runNow')}
                  </button>
                  <button
                    className="icon-button danger"
                    title={t('schedules:schedulesPage.deleteTask')}
                    onClick={remove}
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>
              <label className="field-label">
                Prompt
                <textarea
                  value={draft.prompt}
                  onChange={(event) => updateDraft({ prompt: event.target.value })}
                />
              </label>
              <ScheduleWorkspaceField value={draft.cwd} onChange={(cwd) => updateDraft({ cwd })} />
              <div className="form-grid three">
                <label className="field-label">
                  {t('schedules:schedulesPage.frequency')}
                  <span className="select-wrap">
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
                    <ChevronDown size={13} />
                  </span>
                </label>
                {draft.frequency === 'interval' ? (
                  <label className="field-label">
                    {t('schedules:schedulesPage.runInterval')}
                    <span className="schedule-interval-input">
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
                  </label>
                ) : (
                  <label className="field-label">
                    {t('schedules:schedulesPage.time')}
                    <input
                      type="time"
                      value={draft.time}
                      onChange={(event) => updateDraft({ time: event.target.value })}
                    />
                  </label>
                )}
                <label className="field-label">
                  {t('schedules:schedulesPage.timeZone')}
                  <span className="select-wrap">
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
                    <ChevronDown size={13} />
                  </span>
                </label>
                <ScheduleExecutionModeField
                  value={draft.executionMode}
                  onChange={(executionMode) => updateDraft({ executionMode })}
                />
              </div>
              <div className="schedule-notification-section">
                <div>
                  <strong>{t('schedules:schedulesPage.notificationChannels')}</strong>
                  <button type="button" className="text-button" onClick={openNotificationSettings}>
                    {t('schedules:schedulesPage.editTemplates')}
                  </button>
                </div>
                <p>{t('schedules:schedulesPage.notificationChannelsHelp')}</p>
                <div className="schedule-notification-targets">
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
                <div className="schedule-notify-mode">
                  <button
                    type="button"
                    className={`schedule-notification-chip ${draft.notifyOn === 'failure' ? 'selected' : ''}`}
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
              <div className="form-footer">
                <span>
                  {selected.lastRunAt
                    ? t('schedules:schedulesPage.lastRunTime', {
                        time: relativeTime(selected.lastRunAt, language),
                      })
                    : t('schedules:schedulesPage.nextRunTime', {
                        time: nextRunLabel(selected, language),
                      })}
                </span>
                <button
                  className="button dark"
                  disabled={saving || !draft.prompt.trim()}
                  onClick={save}
                >
                  {saving ? <RefreshCw className="spin" size={14} /> : null}
                  {saving
                    ? t('schedules:schedulesPage.saving')
                    : t('schedules:schedulesPage.saveTask')}
                </button>
              </div>
            </Panel>
            <Panel>
              <SectionTitle title={t('schedules:schedulesPage.recentRuns')} />
              {runs.length ? (
                runs.map((item) => (
                  <div className={`schedule-run-row ${item.status}`} key={item.id}>
                    {item.status === 'running' ? (
                      <RefreshCw className="spin" size={15} />
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
                <div className="channel-route-empty compact">
                  <StarOrbit size={32} />
                  <strong>{t('schedules:schedulesPage.noRunHistory')}</strong>
                </div>
              )}
            </Panel>
          </div>
        ) : (
          <CreateSchedulePanel
            notificationTargets={data.notificationTargets}
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

function CreateSchedulePanel({ notificationTargets, defaultCwd, onCreated }: ScheduleCreatorProps) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [prompt, setPrompt] = useState('')
  const [cwd, setCwd] = useState(defaultCwd || '')
  const [frequency, setFrequency] = useState<ScheduleFrequency>('daily')
  const [time, setTime] = useState('09:00')
  const [timezone, setTimezone] = useState('Asia/Hong_Kong')
  const [intervalValue, setIntervalValue] = useState(1)
  const [intervalUnit, setIntervalUnit] = useState<IntervalUnit>('hours')
  const [executionMode, setExecutionMode] = useState<ScheduleExecutionMode>('full-access')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
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
            prompt,
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
      <div className="card-head">
        <div>
          <h2>{t('schedules:schedulesPage.newScheduledTask')}</h2>
          <p>
            {t(
              'schedules:schedulesPage.youCanContinueEditingTheRunTimeAndNotificationChannelsAfterCreation',
            )}
          </p>
        </div>
      </div>
      <label className="field-label">
        {t('schedules:schedulesPage.taskName')}
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          placeholder={t('schedules:schedulesPage.forExampleDailyCodeReview')}
        />
      </label>
      <label className="field-label">
        Prompt
        <textarea
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
          placeholder={t('schedules:schedulesPage.describeTheWorkTheAgentShouldCompleteEachTime')}
        />
      </label>
      <ScheduleWorkspaceField value={cwd} onChange={setCwd} />
      <div className="form-grid three">
        <label className="field-label">
          {t('schedules:schedulesPage.frequency')}
          <span className="select-wrap">
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
            <ChevronDown size={13} />
          </span>
        </label>
        {frequency === 'interval' ? (
          <label className="field-label">
            {t('schedules:schedulesPage.runInterval')}
            <span className="schedule-interval-input">
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
          </label>
        ) : (
          <label className="field-label">
            {t('schedules:schedulesPage.time')}
            <input type="time" value={time} onChange={(event) => setTime(event.target.value)} />
          </label>
        )}
        <label className="field-label">
          {t('schedules:schedulesPage.timeZone')}
          <span className="select-wrap">
            <AppSelect value={timezone} onChange={(event) => setTimezone(event.target.value)}>
              {TIMEZONES.map((item) => (
                <option value={item} key={item}>
                  {item}
                </option>
              ))}
            </AppSelect>
            <ChevronDown size={13} />
          </span>
        </label>
        <ScheduleExecutionModeField value={executionMode} onChange={setExecutionMode} />
      </div>
      {error && (
        <div className="config-error">
          <AlertTriangle size={13} />
          {error}
        </div>
      )}
      <div className="form-footer">
        <span>{t('schedules:schedulesPage.enabledAutomaticallyAfterCreation')}</span>
        <button
          className="button dark"
          disabled={saving || !name.trim() || !prompt.trim()}
          onClick={create}
        >
          {saving ? <RefreshCw className="spin" size={14} /> : <Plus size={14} />}
          {saving ? t('schedules:schedulesPage.creating') : t('schedules:schedulesPage.createTask')}
        </button>
      </div>
    </Panel>
  )
}

function CreateScheduleModal({
  notificationTargets,
  defaultCwd,
  onClose,
  onCreated,
}: CreateScheduleModalProps) {
  const { t } = useI18n()
  const [name, setName] = useState('')
  const [prompt, setPrompt] = useState('')
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
            prompt,
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
      className="modal-backdrop"
      onMouseDown={(event) => event.target === event.currentTarget && onClose()}
    >
      <form className="modal" onSubmit={submit}>
        <div className="card-head">
          <div>
            <h2>{t('schedules:schedulesPage.newScheduledTask')}</h2>
            <p>
              {t(
                'schedules:schedulesPage.youCanContinueSettingTheRunTimeAndNotificationChannelsAfterCreation',
              )}
            </p>
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label={t('schedules:schedulesPage.closeDialog')}
            onClick={onClose}
          >
            <X size={17} />
          </button>
        </div>
        <label className="field-label">
          {t('schedules:schedulesPage.taskName')}
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder={t('schedules:schedulesPage.forExampleDailyCodeReview')}
          />
        </label>
        <label className="field-label">
          Prompt
          <textarea
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
            placeholder={t('schedules:schedulesPage.describeTheWorkTheAgentShouldCompleteEachTime')}
          />
        </label>
        <ScheduleWorkspaceField value={cwd} onChange={setCwd} />
        <ScheduleExecutionModeField value={executionMode} onChange={setExecutionMode} />
        {error && (
          <div className="config-error">
            <AlertTriangle size={13} />
            {error}
          </div>
        )}
        <div className="modal-actions">
          <button type="button" className="button secondary" onClick={onClose}>
            {t('schedules:schedulesPage.cancel')}
          </button>
          <button className="button primary" disabled={saving || !name.trim() || !prompt.trim()}>
            {saving ? <RefreshCw className="spin" size={14} /> : <Plus size={14} />}
            {saving
              ? t('schedules:schedulesPage.creating')
              : t('schedules:schedulesPage.createTask')}
          </button>
        </div>
      </form>
    </div>
  )
}
