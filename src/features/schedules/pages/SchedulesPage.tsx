// 定时计划页：查看/创建/启停周期任务，展示下次执行时间与历史。
// 仅负责状态编排和数据请求，表单控件委托给 components/ 下的子组件。
import { useCallback, useEffect, useMemo, useState } from 'react'
import { AlertTriangle, CheckCircle2, Play, RefreshCw, Trash2 } from 'lucide-react'
import {
  AppCard as Panel,
  AppSectionTitle as SectionTitle,
  AppSwitch as Toggle,
  StatusBadge as Badge,
  AppCardHeader,
  AppError,
  AppEmptyState,
} from '@/components/ui/app-primitives'
import { AppSelect } from '@/components/common/AppSelect'
import { useI18n } from '@/app/i18n/use-i18n'
import { StarOrbit } from '@/components/common/StarOrbit'
import { apiJson } from '@/lib/http/api'
import { relativeTime } from '@/lib/format/format'
import { usePagePrimaryAction } from '@/hooks/usePagePrimaryAction'
import { Button } from '@/components/ui/button'
import { FieldLabel } from '@/components/ui/field'
import { CreateScheduleModal } from '@/features/schedules/components/CreateScheduleModal'
import { CreateSchedulePanel } from '@/features/schedules/components/CreateSchedulePanel'
import { ScheduleExecutionModeField } from '@/features/schedules/components/ScheduleExecutionModeField'
import { ScheduleTargetFields } from '@/features/schedules/components/ScheduleTargetFields'
import { ScheduleWorkspaceField } from '@/features/schedules/components/ScheduleWorkspaceField'
import { FREQUENCIES, INTERVAL_UNITS, TARGETS, TIMEZONES } from '@/features/schedules/model/schedule-constants'
import {
  frequencyLabel,
  intervalUnitLabel,
  nextRunLabel,
  notificationTargetLabel,
  scheduleTargetValid,
  taskDraft,
} from '@/features/schedules/model/schedule-utils'
import type {
  IntervalUnit,
  NotificationTarget,
  ScheduleDraft,
  ScheduleFrequency,
  ScheduleMutationResult,
  SchedulesData,
} from '@/features/schedules/model/schedule-types'
import type { Notify } from '@/app/routes/route-context'
import type { ConfirmDialogOptions } from '@/hooks/useAppDialog'

type SchedulesPageProps = {
  openNotificationSettings: () => void
  notify: Notify
  registerPrimaryAction: (action: () => void) => () => void
  
  requestConfirm: (options?: ConfirmDialogOptions) => Promise<boolean>
}

export function SchedulesPage({
  notify,
  registerPrimaryAction,
  
  requestConfirm,
  openNotificationSettings: _openNotificationSettings,
}: SchedulesPageProps) {
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
      <div className="split-list-detail grid min-h-[100%] min-w-0 grid-cols-[minmax(0,310px)_minmax(0,1fr)] gap-[12px] overflow-x-hidden max-[900px]:grid-cols-1 max-[650px]:gap-[8px]">
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
              <AppCardHeader className="max-[650px]:flex-wrap max-[650px]:items-start">
                <h2 className="min-w-0 break-words">{draft.name}</h2>
                <div className="flex min-w-0 flex-wrap items-center gap-[5px] max-[650px]:w-full">
                  <Toggle
                    value={draft.enabled}
                    onChange={(enabled) => updateDraft({ enabled })}
                    ariaLabel={draft.name}
                  />
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
                    aria-label={t('schedules:schedulesPage.deleteTask')}
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
                    {TIMEZONES.map((item) => (
                      <option value={item} key={item}>
                        {item}
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
              <SectionTitle title={t('schedules:schedulesPage.notifications')} />
              <div className="notification-grid [&_>_label]:[&.on]:bg-[var(--accent-soft)] [&_>_label]:[&.on]:border-[var(--accent)] [&_>_label]:[&.on]:text-[var(--accent)] grid gap-[6px] grid-cols-[repeat(2,minmax(0,1fr))] max-[650px]:grid-cols-[1fr]">
                {Object.entries(data.notificationTargets).map(([target, config]) => {
                  const { Icon } = TARGETS[target as NotificationTarget] || TARGETS.browser
                  const enabled = draft.notifications.includes(target as NotificationTarget)
                  return (
                    <button
                      className={`notification-chip [&.on]:border-[var(--accent)] [&.on]:bg-[var(--accent-soft)] [&.on]:text-[var(--accent)] flex items-center gap-[7px] [border:1px_solid_var(--stroke)] rounded-[var(--r-sm)] [padding:7px_10px] text-[13px] transition-colors ${enabled ? 'on' : ''}`}
                      key={target}
                      onClick={() => toggleNotification(target as NotificationTarget)}
                      disabled={!config.enabled}
                    >
                      <Icon size={14} />
                      {notificationTargetLabel(target as NotificationTarget, t)}
                      {!config.enabled && (
                        <small className="text-[var(--text-muted)] text-[11px] ml-auto">
                          {t('schedules:schedulesPage.unconfigured')}
                        </small>
                      )}
                    </button>
                  )
                })}
              </div>
              {draft.notifications.length > 0 && (
                <FieldLabel variant="control">
                  {t('schedules:schedulesPage.notifyOn')}
                  <AppSelect
                    value={draft.notifyOn}
                    onChange={(event) =>
                      updateDraft({ notifyOn: event.target.value as 'always' | 'failure' })
                    }
                  >
                    <option value="always">
                      {t('schedules:schedulesPage.onCompletionAndFailure')}
                    </option>
                    <option value="failure">{t('schedules:schedulesPage.onFailureOnly')}</option>
                  </AppSelect>
                </FieldLabel>
              )}
              {draft.targetType === 'prompt' && (
                <div className="[margin-top:10px]">
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
              )}
            </Panel>
            <Panel>
              <SectionTitle title={t('schedules:schedulesPage.recentRuns')} />
              {runs.length ? (
                runs.map((item) => (
                  <div
                    className={`schedule-run-row [&_>_svg]:text-[var(--text-muted)] [&.completed_>_svg]:text-[var(--success)] [&.failed_>_svg]:text-[var(--danger)] [&.running_>_svg]:text-[var(--star-strong)] [&_>_span]:flex [&_>_span]:min-w-0 [&_>_span]:flex-col [&_>_span]:gap-[4px] [&_strong]:[display:-webkit-box] [&_strong]:overflow-hidden [&_strong]:text-[12px] [&_strong]:leading-[1.4] [&_strong]:[-webkit-box-orient:vertical] [&_strong]:[-webkit-line-clamp:2] [&_small]:text-[var(--text-muted)] [&_small]:text-[13px] [&_a]:text-[var(--text-soft)] [&_a]:text-[13px] [&_a]:[text-decoration:underline] [&_a]:[text-underline-offset:2px] [&_>_em]:text-[var(--text-secondary)] [&_>_em]:text-[12px] [&_>_em]:[font-style:normal] [&_>_em]:whitespace-nowrap grid grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-[9px] [border-top:1px_solid_var(--stroke-soft)] [padding:10px_2px] ${item.status}`}
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
