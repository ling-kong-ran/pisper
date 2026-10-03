// 定时计划领域类型：任务、草稿、运行记录和通知渠道。
export type NotificationTarget = 'browser' | 'feishu' | 'weixin' | 'qq' | 'telegram'
export type ScheduleFrequency = 'interval' | 'daily' | 'weekly' | 'monthly'
export type IntervalUnit = 'minutes' | 'hours' | 'days'
export type ScheduleExecutionMode = 'full-access'
export type ScheduleTargetType = 'prompt' | 'workflow'
export type ScheduleStatus = 'idle' | 'running' | 'completed' | 'failed' | 'interrupted'

export type ScheduleWorkflowInput = {
  id: string
  name: string
  label: string
  type: 'string' | 'text' | 'number' | 'boolean'
  required: boolean
  defaultValue: unknown
  description?: string
}

export type ScheduleWorkflow = {
  id: string
  name: string
  description: string
  revision: number
  inputs: ScheduleWorkflowInput[]
}

export type ScheduleTask = {
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

export type ScheduleDraft = Pick<
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

export type ScheduleRun = {
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

export type NotificationTargets = Record<NotificationTarget, { enabled: boolean }>

export type SchedulesData = {
  tasks: ScheduleTask[]
  runs: ScheduleRun[]
  notificationTargets: NotificationTargets
  workflows: ScheduleWorkflow[]
  defaultCwd: string
}

export type ScheduleMutationResult = { task: ScheduleTask; state: SchedulesData }
