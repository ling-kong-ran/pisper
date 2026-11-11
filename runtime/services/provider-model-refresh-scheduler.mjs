// 模型目录空闲刷新：启动时已经用各 Provider 保存的密钥刷新过一次模型列表，但桌面端常常
// 一开几天，之后 Provider 新上的模型、pi.dev 新收录的上下文窗口都不会再进来。
// 这里定时检查，只在没有会话或子代理在跑时才动手：先按需重抓过期的 pi.dev 窗口数据，
// 再隔固定间隔复用启动时同一个刷新入口。停用的 Provider 和用户删掉的模型
// 由那个入口和模型目录的排除记录处理，这里不另做一套规则。

export const MODEL_LIST_REFRESH_INTERVAL_MS = 6 * 60 * 60 * 1000
export const MODEL_METADATA_MAX_AGE_MS = 7 * 24 * 60 * 60 * 1000
export const MODEL_REFRESH_IDLE_CHECK_MS = 5 * 60 * 1000

export class ProviderModelRefreshScheduler {
  /**
   * @param {object} options
   * @param {() => boolean} options.isBusy 是否有会话、准备中的运行或子代理在跑
   * @param {() => Promise<unknown>} options.refreshProviderModels 启动时同一个模型列表刷新入口
   * @param {(maxAgeMs: number) => Promise<unknown> | null} [options.refreshModelMetadata]
   */
  constructor({
    isBusy,
    refreshProviderModels,
    refreshModelMetadata = () => null,
    intervalMs = MODEL_LIST_REFRESH_INTERVAL_MS,
    metadataMaxAgeMs = MODEL_METADATA_MAX_AGE_MS,
    checkIntervalMs = MODEL_REFRESH_IDLE_CHECK_MS,
    now = () => Date.now(),
    timers = { setInterval, clearInterval },
  }) {
    this.isBusy = isBusy
    this.refreshProviderModels = refreshProviderModels
    this.refreshModelMetadata = refreshModelMetadata
    this.intervalMs = intervalMs
    this.metadataMaxAgeMs = metadataMaxAgeMs
    this.checkIntervalMs = checkIntervalMs
    this.now = now
    this.timers = timers
    this.timer = null
    this.running = null
    this.disposed = false
    this.lastProviderRefreshAt = 0
  }

  // 启动时那次刷新由初始化流程负责，所以第一次定时刷新从现在起算一个间隔。
  start() {
    if (this.timer || this.disposed) return
    this.lastProviderRefreshAt = this.now()
    this.timer = this.timers.setInterval(() => void this.tick(), this.checkIntervalMs)
    this.timer?.unref?.()
  }

  busy() {
    try {
      return Boolean(this.isBusy())
    } catch {
      // 判断不了就当作忙：宁可推迟一轮，也不在运行中改模型目录。
      return true
    }
  }

  // 一次检查：忙或上一次还没结束时跳过；返回本次刷新的 Promise，便于测试和关闭时等待。
  tick() {
    if (this.disposed) return null
    if (this.running) return this.running
    if (this.busy()) return null
    const run = this.refresh().finally(() => {
      if (this.running === run) this.running = null
    })
    this.running = run
    return run
  }

  async refresh() {
    try {
      await this.refreshModelMetadata(this.metadataMaxAgeMs)
    } catch {
      console.warn('Model metadata refresh failed; will retry later.')
    }
    if (this.disposed || this.now() - this.lastProviderRefreshAt < this.intervalMs) return
    // 元数据抓取期间可能有新会话开跑，动模型目录前再确认一次。
    if (this.busy()) return
    this.lastProviderRefreshAt = this.now()
    try {
      await this.refreshProviderModels()
    } catch {
      // 单个 Provider 的失败由刷新入口自己收集；这里只兜住整体异常，下个间隔再试。
      console.warn('Model list refresh failed; will retry later.')
    }
  }

  async dispose() {
    this.disposed = true
    if (this.timer) this.timers.clearInterval(this.timer)
    this.timer = null
    await this.running?.catch(() => {})
  }
}
