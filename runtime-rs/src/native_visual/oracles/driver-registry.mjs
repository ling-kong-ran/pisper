// 驱动注册表：按协议名映射到具体驱动实现（Google/xAI/New API/OpenAI 兼容），
// 驱动模块懒加载，首次使用时才 import。
import { isNewAPIProvider } from './protocol-detection.mjs'

const DRIVER_LOADERS = new Map([
  ['google-image', () => import('./google.mjs').then((module) => module.generateGoogle)],
  ['google-video', () => import('./google.mjs').then((module) => module.generateGoogle)],
  ['xai-image', () => import('./xai.mjs').then((module) => module.generateXAI)],
  ['xai-video', () => import('./xai.mjs').then((module) => module.generateXAI)],
  ['new-api-image', () => import('./new-api.mjs').then((module) => module.generateNewAPI)],
  ['new-api-video', () => import('./new-api.mjs').then((module) => module.generateNewAPI)],
  [
    'openai-image',
    () => import('./openai-compatible.mjs').then((module) => module.generateOpenAICompatible),
  ],
  [
    'openai-video',
    () => import('./openai-compatible.mjs').then((module) => module.generateOpenAICompatible),
  ],
  [
    'openrouter-image',
    () => import('./openai-compatible.mjs').then((module) => module.generateOpenAICompatible),
  ],
])

const loadedDrivers = new Map()

function loadDriver(name) {
  if (!loadedDrivers.has(name)) {
    const loader = DRIVER_LOADERS.get(name)
    if (!loader) return null
    loadedDrivers.set(name, loader())
  }
  return loadedDrivers.get(name)
}

export async function runVisualDriver(model, request, options) {
  const detectedDriver =
    model.driver.startsWith('xai-') && (await isNewAPIProvider(model.baseUrl))
      ? `new-api-${model.kind}`
      : model.driver
  const driver = await loadDriver(detectedDriver)
  if (!driver) throw new Error(`不支持的视觉接口驱动：${model.driver}`)
  return driver(model, request, options)
}
