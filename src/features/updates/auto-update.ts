// 自动更新策略：启动延迟 + 周期检查的定时配置，
// 检查中/下载中/已下载等状态阻塞自动重检。
export const DESKTOP_UPDATE_INITIAL_DELAY_MS = 15_000
export const DESKTOP_UPDATE_INTERVAL_MS = 6 * 60 * 60_000

const AUTOMATIC_CHECK_BLOCKED_STATES = new Set(['checking', 'downloading', 'downloaded'])

type UpdateCheckScheduler = Pick<
  Window,
  'setTimeout' | 'clearTimeout' | 'setInterval' | 'clearInterval'
>

export function shouldAutomaticallyCheckForUpdates(state: string) {
  return !AUTOMATIC_CHECK_BLOCKED_STATES.has(state)
}

export function scheduleDesktopUpdateChecks(
  check: () => Promise<unknown> | unknown,
  scheduler: UpdateCheckScheduler = window,
) {
  let checking = false
  let stopped = false

  const run = () => {
    if (checking || stopped) return
    checking = true
    void Promise.resolve()
      .then(check)
      .catch(() => {})
      .finally(() => {
        checking = false
      })
  }

  const initialTimer = scheduler.setTimeout(run, DESKTOP_UPDATE_INITIAL_DELAY_MS)
  const intervalTimer = scheduler.setInterval(run, DESKTOP_UPDATE_INTERVAL_MS)

  return () => {
    stopped = true
    scheduler.clearTimeout(initialTimer)
    scheduler.clearInterval(intervalTimer)
  }
}
