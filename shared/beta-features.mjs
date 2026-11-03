// 实验功能清单与开关的纯逻辑：Web 与 Runtime 共用这一份，设计见 src/features/labs/README.md。
// - 清单是唯一来源；文案不在这里，标题、说明、“关掉后会怎样”用 i18n 键 labs:feature.<id>.*。
// - id 一经使用不得复用：功能毕业或放弃时从清单删除，配置里的旧键读取时忽略、写入时原样保留，
//   这样降级到旧版本再升级回来，开关也不会丢，也不会被新功能意外继承。
// - 开关只表示“用户想用”；能不能用由连接的 Runtime 的 capability 决定，与客户端设备无关。

export const BETA_FEATURE_STAGES = Object.freeze(/** @type {const} */ (['experimental', 'preview']))

const ID_PATTERN = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)*$/
const MAX_ID_CHARS = 48
// 配置里最多保留的开关键数：防止被写入的垃圾键无限增长。
export const MAX_STORED_BETA_FLAGS = 64

/**
 * @typedef {(typeof BETA_FEATURE_STAGES)[number]} BetaFeatureStage
 * @typedef {{
 *   id: string,
 *   stage: BetaFeatureStage,
 *   requires: readonly string[],
 *   feedbackUrl: string,
 * }} BetaFeature
 * @typedef {{
 *   id: string,
 *   stage: BetaFeatureStage,
 *   feedbackUrl: string,
 *   enabled: boolean,
 *   available: boolean,
 *   unavailableReason: 'runtime' | null,
 * }} BetaFeatureState
 */

/** @type {readonly BetaFeature[]} */
export const BETA_FEATURES = Object.freeze([
  {
    id: 'bots',
    stage: 'experimental',
    requires: ['bots'],
    feedbackUrl: 'https://github.com/ling-kong-ran/pisper/issues',
  },
])

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : {}
}

/** @param {unknown} id */
export function isBetaFeatureId(id) {
  return typeof id === 'string' && id.length <= MAX_ID_CHARS && ID_PATTERN.test(id)
}

// 清单自检：测试与启动时都可调用，任何一条不合规都直接抛错，不带病上线。
/** @param {readonly BetaFeature[]} catalog */
export function validateBetaFeatureCatalog(catalog) {
  const seen = new Set()
  for (const feature of catalog) {
    if (!isBetaFeatureId(feature.id)) throw new Error(`实验功能 id 不合法：${feature.id}`)
    if (seen.has(feature.id)) throw new Error(`实验功能 id 重复：${feature.id}`)
    seen.add(feature.id)
    if (!BETA_FEATURE_STAGES.includes(feature.stage))
      throw new Error(`实验功能 ${feature.id} 的阶段不合法：${feature.stage}`)
    if (
      !Array.isArray(feature.requires) ||
      feature.requires.some((item) => typeof item !== 'string')
    )
      throw new Error(`实验功能 ${feature.id} 的 requires 必须是字符串数组。`)
    if (!/^https:\/\/github\.com\//.test(feature.feedbackUrl))
      throw new Error(`实验功能 ${feature.id} 的反馈链接必须指向 GitHub。`)
  }
  return catalog
}

// 配置里存的开关（不可信）→ 只含合法 id 与布尔值的对象。未知 id 保留，供以后的版本读取。
/** @param {unknown} value @returns {Record<string, boolean>} */
export function normalizeStoredBetaFlags(value) {
  /** @type {Record<string, boolean>} */
  const flags = {}
  for (const [id, enabled] of Object.entries(record(value))) {
    if (Object.keys(flags).length >= MAX_STORED_BETA_FLAGS) break
    if (isBetaFeatureId(id) && typeof enabled === 'boolean') flags[id] = enabled
  }
  return flags
}

// PATCH 请求体（不可信）→ 合并后的开关。只接受清单里的 id 与布尔值，任何非法输入整体拒绝。
/**
 * @param {Record<string, boolean>} stored
 * @param {unknown} patch
 * @param {readonly BetaFeature[]} [catalog]
 * @returns {Record<string, boolean>}
 */
export function applyBetaFlagPatch(stored, patch, catalog = BETA_FEATURES) {
  if (patch === null || typeof patch !== 'object' || Array.isArray(patch))
    throw new Error('实验功能开关必须是 JSON 对象。')
  const entries = Object.entries(patch)
  if (entries.length === 0) throw new Error('没有要修改的实验功能。')
  const known = new Set(catalog.map((feature) => feature.id))
  for (const [id, enabled] of entries) {
    if (!known.has(id)) throw new Error(`没有这个实验功能：${id.slice(0, MAX_ID_CHARS)}`)
    if (typeof enabled !== 'boolean') throw new Error(`实验功能 ${id} 的开关必须是布尔值。`)
  }
  return { ...stored, ...Object.fromEntries(entries) }
}

// 对外的状态投影：清单顺序；available 只看 Runtime 的 capability，缺项按不可用处理。
/**
 * @param {Record<string, boolean>} flags
 * @param {Record<string, boolean | undefined> | null | undefined} features
 * @param {readonly BetaFeature[]} [catalog]
 * @returns {BetaFeatureState[]}
 */
export function projectBetaFeatures(flags, features, catalog = BETA_FEATURES) {
  const available = record(features)
  return catalog.map((feature) => {
    const supported = feature.requires.every((name) => available[name] === true)
    return {
      id: feature.id,
      stage: feature.stage,
      feedbackUrl: feature.feedbackUrl,
      enabled: flags[feature.id] === true,
      available: supported,
      unavailableReason: supported ? null : 'runtime',
    }
  })
}

// 功能实际生效：用户打开了，且 Runtime 支持。服务在做决定的那一刻调用，不缓存。
/**
 * @param {Record<string, boolean>} flags
 * @param {Record<string, boolean | undefined> | null | undefined} features
 * @param {string} id
 * @param {readonly BetaFeature[]} [catalog]
 */
export function isBetaFeatureActive(flags, features, id, catalog = BETA_FEATURES) {
  const feature = catalog.find((item) => item.id === id)
  if (!feature || flags[id] !== true) return false
  const available = record(features)
  return feature.requires.every((name) => available[name] === true)
}
