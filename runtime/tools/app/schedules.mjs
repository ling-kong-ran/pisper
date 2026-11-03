// 定时任务工具：schedule_list（只读）+ schedule_manage（新建/修改/暂停/恢复/删除/立即运行）。
// 定时任务会在之后无人值守、以完全访问权限运行，所以新建、修改、恢复与立即运行在非完全访问会话里
// 一律逐次确认且不受持久允许规则影响（见 session-permission-service 的 schedule_manage 分支）；
// 子代理与无人值守的运行拿不到这两个工具，避免任务自我复制。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

export const SCHEDULE_TOOL_NAMES = Object.freeze(['schedule_list', 'schedule_manage'])

export const manifests = [
  {
    id: 'schedule_list',
    name: 'Schedule List',
    category: 'schedules',
    risk: 'low',
    description: 'List Pisper scheduled tasks with their timing, next run and last result.',
    scope: 'Application-level scheduled tasks',
    capability: 'Read scheduled tasks without changing them',
    source: 'app',
  },
  {
    id: 'schedule_manage',
    name: 'Schedule Manage',
    category: 'schedules',
    risk: 'high',
    description:
      'Create, update, pause, resume, delete or run Pisper scheduled tasks, including one-time tasks that disable themselves after running.',
    scope: 'Application-level scheduled tasks',
    capability:
      'Create tasks that later run unattended with full access in an isolated session; creating, changing, resuming or running them needs confirmation unless the session already has full access',
    source: 'app',
  },
]

const FREQUENCY = Type.Union(
  ['once', 'interval', 'daily', 'weekly', 'monthly'].map((value) => Type.Literal(value)),
  {
    description:
      'once = a single run at date + time; interval = every intervalValue intervalUnit; daily / weekly / monthly at time.',
  },
)
const NOTIFICATION = Type.Union(
  ['browser', 'feishu', 'weixin', 'qq', 'telegram'].map((value) => Type.Literal(value)),
)
const timing = {
  date: Type.Optional(
    Type.String({
      pattern: '^\\d{4}-\\d{2}-\\d{2}$',
      description: 'YYYY-MM-DD in the task timezone. Required for frequency "once".',
    }),
  ),
  time: Type.Optional(
    Type.String({
      pattern: '^([01]\\d|2[0-3]):[0-5]\\d$',
      description: 'HH:MM (24h) in the task timezone.',
    }),
  ),
  timezone: Type.Optional(
    Type.String({
      minLength: 1,
      maxLength: 64,
      description: 'IANA timezone. Omit to use the timezone of the machine running Pisper.',
    }),
  ),
  intervalValue: Type.Optional(Type.Integer({ minimum: 1, maximum: 10_000 })),
  intervalUnit: Type.Optional(
    Type.Union(['minutes', 'hours', 'days'].map((value) => Type.Literal(value))),
  ),
  dayOfWeek: Type.Optional(Type.Integer({ minimum: 0, maximum: 6, description: '0 = Sunday.' })),
  dayOfMonth: Type.Optional(Type.Integer({ minimum: 1, maximum: 28 })),
}
const details = {
  name: Type.String({ minLength: 1, maxLength: 120 }),
  prompt: Type.String({
    minLength: 1,
    maxLength: 20_000,
    description:
      'Self-contained instructions for the future unattended run: include paths, the goal and what to report.',
  }),
  cwd: Type.Optional(
    Type.String({
      minLength: 1,
      maxLength: 4_000,
      description: 'Working directory. Omit to use the current workspace.',
    }),
  ),
  notifications: Type.Optional(
    Type.Array(NOTIFICATION, {
      maxItems: 5,
      description: 'Where to send the result. Only channels the user has connected will deliver.',
    }),
  ),
}
const id = Type.String({ minLength: 1, maxLength: 200 })
// 定时运行已发布的工作流：与 prompt 二选一。
const workflowTarget = {
  workflowId: Type.Optional(
    Type.String({
      minLength: 1,
      maxLength: 200,
      description: 'Run this published workflow instead of a prompt.',
    }),
  ),
  workflowInputs: Type.Optional(Type.Record(Type.String({ maxLength: 120 }), Type.Unknown())),
}

function localTimeZone() {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

// 当前时刻随结果一起返回：模型据此把“明天早上 9 点”换算成具体日期。
function now(timeZone) {
  const at = new Date()
  return {
    iso: at.toISOString(),
    timezone: timeZone,
    local: new Intl.DateTimeFormat('sv-SE', {
      timeZone,
      dateStyle: 'short',
      timeStyle: 'short',
    }).format(at),
  }
}

function publicTask(task) {
  return {
    id: task.id,
    name: task.name,
    enabled: task.enabled,
    target: task.targetType === 'workflow' ? { workflowId: task.workflowId } : 'prompt',
    prompt: task.targetType === 'prompt' ? String(task.prompt || '').slice(0, 500) : undefined,
    frequency: task.frequency,
    ...(task.frequency === 'once' ? { date: task.date } : {}),
    ...(task.frequency === 'interval'
      ? { intervalValue: task.intervalValue, intervalUnit: task.intervalUnit }
      : { time: task.time }),
    ...(task.frequency === 'weekly' ? { dayOfWeek: task.dayOfWeek } : {}),
    ...(task.frequency === 'monthly' ? { dayOfMonth: task.dayOfMonth } : {}),
    timezone: task.timezone,
    nextRunAt: task.nextRunAt,
    lastRunAt: task.lastRunAt,
    lastStatus: task.lastStatus,
    lastError: task.lastError ? String(task.lastError).slice(0, 300) : undefined,
    notifications: task.notifications,
    createdBy: task.createdBy?.kind || 'user',
  }
}

function resultContent(value) {
  return { content: [{ type: 'text', text: JSON.stringify(value, null, 2) }], details: value }
}

// 只把模型给了的字段写进修改：未提及的字段保持原值。
function pickDefined(params, keys) {
  return Object.fromEntries(
    keys.filter((key) => params[key] !== undefined).map((key) => [key, params[key]]),
  )
}

const TIMING_KEYS = Object.keys(timing)

export function createScheduleListTool({ scheduleRuntime }) {
  return defineTool({
    name: manifests[0].id,
    label: manifests[0].name,
    description: manifests[0].description,
    promptSnippet: 'List Pisper scheduled tasks and the current time',
    promptGuidelines: [
      'Call schedule_list first to get the exact task id before updating, pausing, resuming, deleting or running a task.',
      'The result includes the current time; use it to turn relative times such as "tomorrow at 9" into a date.',
    ],
    parameters: Type.Object({}),
    async execute() {
      const state = await scheduleRuntime.list()
      return resultContent({ now: now(localTimeZone()), tasks: state.tasks.map(publicTask) })
    },
  })
}

export function createScheduleManageTool({ scheduleRuntime, cwd }) {
  return defineTool({
    name: manifests[1].id,
    label: manifests[1].name,
    description: manifests[1].description,
    promptSnippet:
      'Create reminders and one-time or recurring scheduled tasks inside Pisper instead of OS schedulers',
    promptGuidelines: [
      'Use schedule_manage for anything the user wants done later or repeatedly. Never use cron, at, launchd, systemd timers, Windows Task Scheduler or sleep loops through bash for this.',
      'A scheduled run happens later without the user present, in a fresh isolated session with full access. Write the prompt so it stands on its own: paths, goal and what to report.',
      'For a single run use frequency "once" with date (YYYY-MM-DD) and time (HH:MM); the task disables itself after it runs. Times in the past are rejected.',
      'Omit timezone unless the user names one; Pisper then uses the timezone of the machine it runs on.',
      'Creating, changing, resuming or running a task asks the user to confirm unless the session already has full access. Pausing and deleting only stop work.',
      'To run a published workflow on a schedule, give workflowId (and workflowInputs) instead of prompt; find both with workflow_list.',
    ],
    parameters: Type.Union([
      Type.Object({
        action: Type.Literal('create'),
        ...details,
        prompt: Type.Optional(details.prompt),
        ...workflowTarget,
        frequency: FREQUENCY,
        ...timing,
      }),
      Type.Object({
        action: Type.Literal('update'),
        id,
        name: Type.Optional(details.name),
        prompt: Type.Optional(details.prompt),
        ...workflowTarget,
        cwd: details.cwd,
        notifications: details.notifications,
        frequency: Type.Optional(FREQUENCY),
        ...timing,
      }),
      Type.Object({ action: Type.Literal('pause'), id }),
      Type.Object({ action: Type.Literal('resume'), id }),
      Type.Object({ action: Type.Literal('delete'), id }),
      Type.Object({ action: Type.Literal('run_now'), id }),
    ]),
    async execute(_toolCallId, params, signal) {
      if (signal?.aborted) throw new Error('Schedule management was cancelled.')
      const timeZone = localTimeZone()
      let task
      if (params.action === 'create') {
        if (Boolean(params.prompt) === Boolean(params.workflowId))
          throw new Error('Give either prompt or workflowId, not both.')
        task = await scheduleRuntime.create({
          name: params.name,
          ...(params.workflowId
            ? {
                targetType: 'workflow',
                workflowId: params.workflowId,
                workflowInputs: params.workflowInputs || {},
              }
            : { targetType: 'prompt', prompt: params.prompt }),
          cwd: params.cwd || cwd,
          enabled: true,
          frequency: params.frequency,
          ...pickDefined(params, TIMING_KEYS),
          timezone: params.timezone || timeZone,
          notifications: params.notifications || [],
          notifyOn: 'always',
        })
      } else if (params.action === 'update') {
        if (params.prompt && params.workflowId)
          throw new Error('Give either prompt or workflowId, not both.')
        task = await scheduleRuntime.update(params.id, {
          ...pickDefined(params, [
            'name',
            'prompt',
            'workflowId',
            'workflowInputs',
            'cwd',
            'notifications',
            'frequency',
            ...TIMING_KEYS,
          ]),
          // 换目标时一并切换 targetType；只改时间等字段时保持原目标。
          ...(params.workflowId ? { targetType: 'workflow' } : {}),
          ...(params.prompt ? { targetType: 'prompt' } : {}),
        })
      } else if (params.action === 'pause') {
        task = await scheduleRuntime.update(params.id, { enabled: false })
      } else if (params.action === 'resume') {
        task = await scheduleRuntime.update(params.id, { enabled: true })
      } else if (params.action === 'delete') {
        if (!(await scheduleRuntime.remove(params.id)))
          throw new Error('Scheduled task does not exist.')
        return resultContent({ deleted: params.id, now: now(timeZone) })
      } else if (params.action === 'run_now') {
        task = await scheduleRuntime.runNow(params.id)
      }
      if (!task) throw new Error('Scheduled task does not exist.')
      return resultContent({ task: publicTask(task), now: now(timeZone) })
    },
  })
}

export const factories = {
  schedule_list: createScheduleListTool,
  schedule_manage: createScheduleManageTool,
}
