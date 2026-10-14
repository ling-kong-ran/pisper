import { mobileBaseInitialization } from '../runtime-capabilities.mjs'
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
  const initializeMemory = async () => {
    stage('memory')
    if (!runtime.capabilities.features.memory) return
    runtime.initialization.memory = 'initializing'
    try {
      await runtime.memory.init()
      runtime.memory.setSemanticSummarizer(runtime.memorySummarizer)
      runtime.initialization.memory = 'ready'
    } catch (error) {
      runtime.initialization.memory = 'failed'
      throw error
    }
  }
  return {
    stage,
    initializeRequiredMemory: () => (deferMemory ? undefined : initializeMemory()),
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
