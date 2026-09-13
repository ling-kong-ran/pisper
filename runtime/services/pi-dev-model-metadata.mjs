// Pi.dev 模型元数据服务：从 https://pi.dev/models 获取最新模型信息并缓存
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const PI_DEV_MODELS_URL = 'https://pi.dev/models'
const CACHE_TTL_MS = 24 * 60 * 60 * 1000 // 24 小时
const FETCH_TIMEOUT_MS = 10_000 // 10 秒超时

/**
 * 从 pi.dev/models HTML 页面解析模型元数据
 * @param {string} html - HTML 内容
 * @returns {Map<string, {contextWindow: number}>} 模型 ID -> 元数据映射
 */
function parseModelsFromHtml(html) {
  const models = new Map()

  // 匹配表格行：<tr data-model-row="true" data-model-name="..." data-model-id="...">
  // 然后提取上下文窗口：<td ... data-label="Context">1,000,000</td>
  const rowRegex =
    /<tr[^>]*data-model-row="true"[^>]*data-model-id="([^"]+)"[^>]*>([\s\S]*?)<\/tr>/g

  let match
  while ((match = rowRegex.exec(html)) !== null) {
    const modelId = match[1]
    const rowContent = match[2]

    // 提取上下文窗口
    const contextMatch = rowContent.match(/data-label="Context">([0-9,]+)</)
    if (contextMatch) {
      const contextWindow = parseInt(contextMatch[1].replace(/,/g, ''), 10)
      if (!isNaN(contextWindow) && contextWindow > 0) {
        models.set(modelId, { contextWindow })
      }
    }
  }

  return models
}

/**
 * Pi.dev 模型元数据服务
 */
export class PiDevModelMetadataService {
  /**
   * @param {object} options
   * @param {string} options.cachePath - 缓存文件路径
   */
  constructor({ cachePath }) {
    this.cachePath = cachePath
    this.cache = null
    this.lastFetch = 0
  }

  /**
   * 初始化：从缓存加载
   */
  async init() {
    try {
      const cached = await readJson(this.cachePath)
      if (cached?.timestamp && cached?.models) {
        this.cache = new Map(Object.entries(cached.models))
        this.lastFetch = cached.timestamp
      }
    } catch {
      // 缓存文件不存在或损坏，忽略
    }
  }

  /**
   * 同步获取上下文窗口（仅从已加载缓存读取）
   * @param {string} modelId - 模型 ID
   * @returns {number | null}
   */
  getContextWindowSync(modelId) {
    if (!this.cache) return null

    // 精确匹配
    const exact = this.cache.get(modelId)
    if (exact?.contextWindow) return exact.contextWindow

    // 模糊匹配
    const normalizedId = modelId.toLowerCase()
    for (const [cachedId, metadata] of this.cache.entries()) {
      if (
        cachedId.toLowerCase().includes(normalizedId) ||
        normalizedId.includes(cachedId.toLowerCase())
      ) {
        return metadata.contextWindow || null
      }
    }

    return null
  }

  /**
   * 获取模型元数据（带缓存和回退）
   * @param {string} modelId - 模型 ID
   * @returns {{contextWindow: number} | null}
   */
  async getMetadata(modelId) {
    // 如果缓存过期，尝试刷新
    const now = Date.now()
    if (!this.cache || now - this.lastFetch > CACHE_TTL_MS) {
      await this.refresh()
    }

    if (!this.cache) return null

    // 精确匹配
    const exact = this.cache.get(modelId)
    if (exact) return exact

    // 模糊匹配：尝试找到包含该 ID 的模型（不区分大小写）
    const normalizedId = modelId.toLowerCase()
    for (const [cachedId, metadata] of this.cache.entries()) {
      if (
        cachedId.toLowerCase().includes(normalizedId) ||
        normalizedId.includes(cachedId.toLowerCase())
      ) {
        return metadata
      }
    }

    return null
  }

  /**
   * 刷新模型元数据（从 pi.dev 抓取）
   */
  async refresh() {
    try {
      const controller = new AbortController()
      const timeoutId = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS)

      const response = await fetch(PI_DEV_MODELS_URL, { signal: controller.signal })
      clearTimeout(timeoutId)

      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`)
      }

      const html = await response.text()
      const models = parseModelsFromHtml(html)

      if (models.size > 0) {
        this.cache = models
        this.lastFetch = Date.now()

        // 持久化到缓存文件
        await writeJsonAtomic(this.cachePath, {
          timestamp: this.lastFetch,
          models: Object.fromEntries(models),
        })
      }
    } catch (error) {
      // 获取失败时保留旧缓存，记录错误但不抛出
      console.warn(`[PiDevModelMetadata] Failed to fetch from ${PI_DEV_MODELS_URL}:`, error.message)
    }
  }

  /**
   * 手动清除缓存
   */
  clearCache() {
    this.cache = null
    this.lastFetch = 0
  }
}
