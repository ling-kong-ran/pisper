// Pi.dev 模型元数据：模型上下文窗口的持久化数据文件（不是缓存）。
//
// 设计约定：这份数据被当作恒定的参考数据对待——落盘一次即长期有效，
// 不设 TTL、不做失效、不在未命中时触发网络刷新。仅当数据文件不存在
// （首次运行）或损坏时，才在后台抓取一次 https://pi.dev/models 并落盘；
// 抓取失败保持缺省，由上层优先级链（用户显式配置 > 本文件 > 内置元数据 >
// 推断默认值）兜底，下次启动自然重试。
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const PI_DEV_MODELS_URL = 'https://pi.dev/models'
const FETCH_TIMEOUT_MS = 10_000 // 单次抓取超时

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

// 网关会给模型 id 加厂商/路由前缀，分隔符五花八门：`alibaba/qwen3.8-max`、
// `xxx-qwen3.8-max`、`au.anthropic.claude-opus-5`、`tokenhub&glm-5.2` 指的都是
// 同一个模型。按常见分隔符切成 token 后做「尾部序列相等」匹配，前缀差异就不会
// 再让查找退化成 200k 默认值。
const MODEL_ID_SEPARATORS = /[/:@._\-\s&%]+/g

function modelIdTokens(modelId) {
  return String(modelId || '')
    .trim()
    .toLowerCase()
    .split(MODEL_ID_SEPARATORS)
    .filter(Boolean)
}

// 尾部对齐判断：较短一方的全部 token 必须等于较长一方的结尾，
// 保证 `gpt-5.4` 不会命中 `gpt-5.4-mini` / `qwen3.8-max-0902` 这类后缀变体。
function isSuffixAligned(a, b) {
  const shared = Math.min(a.length, b.length)
  for (let i = 1; i <= shared; i++) {
    if (a[a.length - i] !== b[b.length - i]) return false
  }
  return true
}

/**
 * Pi.dev 模型元数据服务：启动时加载落盘数据；文件缺失时后台抓取一次并落盘。
 */
export class PiDevModelMetadataService {
  /**
   * @param {object} options
   * @param {string} options.cachePath - 持久化数据文件路径
   */
  constructor({ cachePath }) {
    this.cachePath = cachePath
    this.models = null // Map<modelId, {contextWindow}>，来自落盘文件
    // 按尾部 token 分组的查找索引；与 this.models 同步赋值，避免两者状态错位。
    this.byTail = null
    this.fetching = null // 进行中的抓取 Promise（防并发）
  }

  // 从落盘数据构建尾部 token 索引（尾部 token 是后缀对齐的必要条件，可先按它分桶）。
  buildIndex(models) {
    this.models = models
    const byTail = new Map()
    for (const [storedId, metadata] of models.entries()) {
      const contextWindow = Number(metadata?.contextWindow) || 0
      if (!contextWindow) continue
      const tokens = modelIdTokens(storedId)
      if (!tokens.length) continue
      const tail = tokens[tokens.length - 1]
      const bucket = byTail.get(tail)
      if (bucket) bucket.push({ tokens, contextWindow })
      else byTail.set(tail, [{ tokens, contextWindow }])
    }
    this.byTail = byTail
  }

  /**
   * 初始化：加载落盘数据；文件不存在或损坏时后台抓取一次。
   */
  async init() {
    try {
      const stored = await readJson(this.cachePath)
      if (stored?.models) {
        this.buildIndex(new Map(Object.entries(stored.models)))
        return
      }
    } catch {
      // 文件不存在或损坏：走一次性抓取
    }
    this.fetchOnceInBackground()
  }

  /**
   * 数据文件缺失时的一次性后台抓取：不阻塞启动，失败不重试
   * （数据保持缺省，由上层优先级链兜底，下次启动自然重试）。
   */
  fetchOnceInBackground() {
    if (this.fetching) return
    this.fetching = this.fetchAndPersist()
      .catch((err) => {
        console.warn('[PiDev] 模型元数据抓取失败:', err.message)
      })
      .finally(() => {
        this.fetching = null
      })
  }

  /**
   * 同步获取上下文窗口（仅从已加载的落盘数据读取）。
   *
   * 匹配规则：查询 id 与落盘 id 都按分隔符切成 token，要求尾部 token 序列相等——
   * 两侧都允许带任意厂商/区域/路由前缀（`xxx/qwen3.8-max`、`xxx-qwen3.8-max` 都能
   * 命中 `qwen3.8-max`），而 `-mini`/`-0902` 等后缀变体不会串进来。
   *
   * 取值规则：直接命中里最短的 token 序列即该模型的「裸名」；同一裸名的所有
   * 前缀形态（openai/、au.anthropic. 等）取最大窗口。pi.dev 上同一模型常因不同
   * 提供方出现互相矛盾的条目（如 gpt-5.4 同时有 272k 与 1050k），按「最大上下文」
   * 语义取最大值，与 models.dev 元数据归一化的既有策略一致，也避免结果依赖
   * Map 迭代顺序或查询侧前缀的写法。
   *
   * @param {string} modelId - 模型 ID（可带任意前缀）
   * @returns {number | null}
   */
  getContextWindowSync(modelId) {
    if (!this.byTail) return null
    const queryTokens = modelIdTokens(modelId)
    if (!queryTokens.length) return null
    const bucket = this.byTail.get(queryTokens[queryTokens.length - 1])
    if (!bucket) return null

    // 第一遍：找出与查询尾部对齐的直接命中，其中最短的就是裸名 token 序列。
    let canonical = null
    for (const entry of bucket) {
      if (!isSuffixAligned(entry.tokens, queryTokens)) continue
      if (!canonical || entry.tokens.length < canonical.length) canonical = entry.tokens
    }
    if (!canonical) return null

    // 第二遍：在裸名的全部前缀形态里取最大窗口。
    let best = 0
    for (const { tokens, contextWindow } of bucket) {
      if (tokens.length < canonical.length) continue
      if (isSuffixAligned(tokens, canonical) && contextWindow > best) best = contextWindow
    }
    return best || null
  }

  /**
   * 抓取 pi.dev/models 并落盘；解析结果为空时保留既有数据不动。
   */
  async fetchAndPersist() {
    const controller = new AbortController()
    const timeoutId = setTimeout(() => controller.abort(), FETCH_TIMEOUT_MS)
    let response
    try {
      response = await fetch(PI_DEV_MODELS_URL, { signal: controller.signal })
    } finally {
      clearTimeout(timeoutId)
    }
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}`)
    }
    const models = parseModelsFromHtml(await response.text())
    if (models.size > 0) {
      this.buildIndex(models)
      await writeJsonAtomic(this.cachePath, { models: Object.fromEntries(models) })
    }
  }
}
