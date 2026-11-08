// schedule_list / schedule_manage 的说明。

export const SCHEDULE_LIST = Object.freeze({
  description: 'List Pisper scheduled tasks with their timing, next run and last result.',
  snippet: 'List Pisper scheduled tasks and the current time',
  guidelines: [
    'Call schedule_list first to get the exact task id before updating, pausing, resuming, deleting or running a task.',
    'The result includes the current time; use it to turn relative times such as "tomorrow at 9" into a date.',
  ],
})

export const SCHEDULE_MANAGE = Object.freeze({
  description:
    'Create, update, pause, resume, delete or run Pisper scheduled tasks, including one-time tasks that disable themselves after running.',
  snippet:
    'Create reminders and one-time or recurring scheduled tasks inside Pisper instead of OS schedulers',
  guidelines: [
    'Use schedule_manage for anything the user wants done later or repeatedly. Never use cron, at, launchd, systemd timers, Windows Task Scheduler or sleep loops through bash for this.',
    'A scheduled run happens later without the user present, in a fresh isolated session with full access. Write the prompt so it stands on its own: paths, goal and what to report.',
    'For a single run use frequency "once" with date (YYYY-MM-DD) and time (HH:MM); the task disables itself after it runs. Times in the past are rejected.',
    'Omit timezone unless the user names one; Pisper then uses the timezone of the machine it runs on.',
    'Creating, changing, resuming or running a task asks the user to confirm unless the session already has full access. Pausing and deleting only stop work.',
    'To run a published workflow on a schedule, give workflowId (and workflowInputs) instead of prompt; find both with workflow_list.',
  ],
  params: {
    frequency:
      'once = a single run at date + time; interval = every intervalValue intervalUnit; daily / weekly / monthly at time.',
    date: 'YYYY-MM-DD in the task timezone. Required for frequency "once".',
    time: 'HH:MM (24h) in the task timezone.',
    timezone: 'IANA timezone. Omit to use the timezone of the machine running Pisper.',
    dayOfWeek: '0 = Sunday.',
    prompt:
      'Self-contained instructions for the future unattended run: include paths, the goal and what to report.',
    cwd: 'Working directory. Omit to use the current workspace.',
    notifications: 'Where to send the result. Only channels the user has connected will deliver.',
    workflowId: 'Run this published workflow instead of a prompt.',
  },
})
