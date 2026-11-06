import { join } from 'node:path'
import { mobileBaseInitialization } from '../runtime-capabilities.mjs'
import { describeStartupError, setAsideDataFile } from '../services/startup-data-check.mjs'
import { createStartupObserver } from '../startup-observer.mjs'

export function prepareRuntimeInitialization(
  runtime,
  { startupObserver = null, initializationMode = 'full', onBaseReady = null } = {},
) {
  const mobileBase = mobileBaseInitialization(initializationMode, runtime.capabilities.profile)
  // 自动化服务可能在初始化期间启动历史任务，必须先满足它们的记忆依赖。
  const deferMemory =
    mobileBase &&
    !['goals', 'plans', 'multiAgent', 'channels', 'workflows', 'schedules'].some(
      (feature) => runtime.capabilities.features[feature],
    )
  const stage = createStartupObserver(startupObserver)
  runtime.initialization = { memory: runtime.capabilities.features.memory ? 'pending' : 'skipped' }
  runtime.startupReport = { setAside: [], failed: [] }
  // 一个服务读不了自己的数据，不该让整个应用起不来。files 是它的数据文件：
  // 失败时改名留底、从默认值重来一次（内存里不留半截状态，之后写盘也不会盖掉原文件）。
  // required 的步骤没有它就没法继续，重试仍失败时才上抛。
  const attempt = async (name, run, { files = [], required = false } = {}) => {
    let failure
    try {
      await run()
      return true
    } catch (error) {
      failure = error
    }
    let moved = false
    for (const file of files) {
      const kept = await setAsideDataFile(join(runtime.dataDir, file)).catch(() => '')
      if (!kept) continue
      moved = true
      runtime.startupReport.setAside.push({ file, reason: 'rejected_by_service' })
    }
    if (moved) {
      try {
        await run()
        return true
      } catch (error) {
        failure = error
      }
    }
    const description = describeStartupError(failure, { dataDir: runtime.dataDir })
    runtime.initialization[name] = 'failed'
    runtime.startupReport.failed.push({ service: name, ...description })
    console.error(`[startup] ${name} 初始化失败：${description.code} ${description.message}`)
    if (required) throw failure
    return false
  }
  const initializeMemory = async () => {
    stage('memory')
    if (!runtime.capabilities.features.memory) return
    runtime.initialization.memory = 'initializing'
    try {
      await runtime.memory.init()
      runtime.startupReport.setAside.push(...(runtime.memory.setAsideFiles ?? []))
      runtime.memory.setSemanticSummarizer(runtime.memorySummarizer)
      // 后台步骤按顺序轮流跑：对账 → 空闲会话批量抽取 → 整理；定时器归记忆运行时所有，dispose 时清掉。
      runtime.memory.setBackgroundExclusions((spaces) =>
        runtime.isBetaFeatureEnabled?.('bots')
          ? []
          : spaces.filter((space) => space.kind === 'bot').map((space) => space.id),
      )
      runtime.memory.startBackground({
        steps: [(signal) => runtime.memoryCapture.flushIdleSessions({ signal })],
      })
      runtime.initialization.memory = 'ready'
    } catch (error) {
      runtime.initialization.memory = 'failed'
      throw error
    }
  }
  return {
    stage,
    attempt,
    initializeRequiredMemory: () => (deferMemory ? undefined : attempt('memory', initializeMemory)),
    async complete() {
      if (deferMemory) {
        // 先发布聊天依赖；显式记忆操作共享此 Promise，不能读取半初始化服务。
        runtime.memoryReady = new Promise((resolve, reject) => {
          setImmediate(() => initializeMemory().then(resolve, reject))
        })
        runtime.memoryReady.catch(() => {})
        onBaseReady?.()
        try {
          await runtime.memoryReady
        } finally {
          void runtime.refreshProviderModels().catch(() => {})
        }
      } else {
        void runtime.refreshProviderModels().catch(() => {})
        onBaseReady?.()
      }
      stage('complete')
    },
  }
}
