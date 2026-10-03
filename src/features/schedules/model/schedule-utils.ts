// 定时计划纯逻辑：标签格式化、校验和草稿生成。
import type { useI18n } from '@/app/i18n/use-i18n'
import type {
  IntervalUnit,
  NotificationTarget,
  ScheduleDraft,
  ScheduleFrequency,
  ScheduleTask,
  ScheduleWorkflow,
} from './schedule-types'

type Translate = ReturnType<typeof useI18n>['t']

export function executionModeHelp(t: Translate) {
  return t('schedules:schedulesPage.fullAccessHelp')
}

export function frequencyLabel(frequency: ScheduleFrequency, t: Translate) {
  const labels: Record<ScheduleFrequency, string> = {
    interval: t('schedules:schedulesPage.atIntervals'),
    daily: t('schedules:schedulesPage.daily'),
    weekly: t('schedules:schedulesPage.weekly'),
    monthly: t('schedules:schedulesPage.monthly'),
  }
  return labels[frequency]
}

export function intervalUnitLabel(unit: IntervalUnit, t: Translate) {
  const labels: Record<IntervalUnit, string> = {
    minutes: t('schedules:schedulesPage.minutes'),
    hours: t('schedules:schedulesPage.hours'),
    days: t('schedules:schedulesPage.days'),
  }
  return labels[unit]
}

export function notificationTargetLabel(target: NotificationTarget, t: Translate) {
  const labels: Record<NotificationTarget, string> = {
    browser: t('schedules:schedulesPage.browserNotifications'),
    feishu: '飞书',
    weixin: '微信',
    qq: 'QQ',
    telegram: 'Telegram',
  }
  return labels[target]
}

export function workflowInputDefaults(workflow?: ScheduleWorkflow) {
  const values: Record<string, unknown> = {}
  if (!workflow) return values
  for (const input of workflow.inputs) {
    values[input.id] = input.defaultValue
  }
  return values
}

export function scheduleTargetValid(
  targetType: string,
  prompt: string,
  workflowId: string,
  workflowInputs: Record<string, unknown>,
  workflows: ScheduleWorkflow[],
) {
  if (targetType === 'prompt') return Boolean(prompt.trim())
  if (!workflowId) return false
  const workflow = workflows.find((item) => item.id === workflowId)
  if (!workflow) return false
  return workflow.inputs
    .filter((input) => input.required)
    .every((input) => {
      const value = workflowInputs[input.id]
      return value !== undefined && value !== null && String(value).trim() !== ''
    })
}

export function taskDraft(task: ScheduleTask): ScheduleDraft {
  const {
    nextRunAt: _nextRunAt,
    lastRunAt: _lastRunAt,
    lastStatus: _lastStatus,
    ...draft
  } = task
  return draft
}

export function nextRunLabel(task: ScheduleTask, locale = 'zh-CN') {
  if (!task.nextRunAt) return null
  try {
    return new Intl.DateTimeFormat(locale, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    }).format(new Date(task.nextRunAt))
  } catch {
    return null
  }
}
