// 原生组件 manifest v2 与实例属性的共用规则：Runtime 落盘前、前端懒加载表单里都按这一份校验。
// 环境中立：不依赖 DOM、React 或 Node API，输入一律按不可信 unknown 处理。
// 错误只给 { path, code, params? }，由前端按 custom-ui:errors.<code> 本地化，协议里不拼自然语言。

export const CUSTOM_UI_SDK_VERSION = 1

// 放置名在 A7 定稿（规格 5.5）：此前没有发布过 board-card / more-page，不需要兼容旧名。
export const COMPONENT_SURFACES = Object.freeze(
  /** @type {const} */ (['overview-card', 'session-panel', 'feature-page', 'floating']),
)
export const OVERVIEW_CARD_SIZES = Object.freeze(/** @type {const} */ (['S', 'M']))
// 全部代理右侧 320 栏里的卡片尺寸（规格 3.4）；宿主与工作室预览共用。
export const OVERVIEW_CARD_DIMENSIONS = Object.freeze({
  S: Object.freeze({ w: 320, h: 120 }),
  M: Object.freeze({ w: 320, h: 248 }),
})
export const FLOATING_DEFAULT_SIZE = Object.freeze({ w: 320, h: 200 })
export const FLOATING_SIZE_LIMITS = Object.freeze({ minW: 160, maxW: 480, minH: 64, maxH: 480 })
export const CAPABILITY_TYPES = Object.freeze(
  /** @type {const} */ (['network', 'notify', 'sessions.read', 'sessions.open', 'storage']),
)
export const STORAGE_QUOTA_DEFAULT_KB = 256
export const STORAGE_QUOTA_MAX_KB = 1024
export const COMPONENT_PROPS_MAX_BYTES = 16 * 1024
export const COMPONENT_INSTANCE_TITLE_MAX = 80

// 组件 id 即目录名：只允许安全的文件名字符，避免路径与 URL 编码问题。
export const COMPONENT_ID_PATTERN = /^[a-z0-9][a-z0-9._-]{0,63}$/
// 旧内置“专注岛”的 id 继续保留：创建与导入都拒绝，避免与回滚后的旧版本撞名（计划 §2.7）。
export const RESERVED_COMPONENT_IDS = Object.freeze(['pisper-island'])

// ctx SDK v1 的成员清单：技能里的 .d.ts、前端 sdk.ts 与契约测试都以它为准。
export const CTX_V1_KEYS = Object.freeze([
  'sdk',
  'React',
  'html',
  'ui',
  'theme',
  'sessions',
  'notify',
  'net',
  'storage',
  'signal',
  'setTimeout',
  'setInterval',
  'clearTimeout',
  'clearInterval',
  'size',
  'placement',
  'session',
  't',
  'log',
  'CapabilityError',
])
// SDK v1 的 ui 子集：名字取 src/components/ui、src/components/app 实际导出的组件。
export const UI_KIT_V1_KEYS = Object.freeze([
  'Button',
  'Badge',
  'Chip',
  'Input',
  'Textarea',
  'Switch',
  'Checkbox',
  'SegmentedControl',
  'Progress',
  'Spinner',
  'Skeleton',
  'Separator',
  'Kbd',
  'Tooltip',
  'TooltipTrigger',
  'TooltipContent',
  'TooltipProvider',
  'StatusGlyph',
  'StatusTag',
  'Empty',
])

export const MANIFEST_LOCALES = Object.freeze(/** @type {const} */ (['zh-CN', 'en-US']))

const FILE_SEGMENT = /^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/
const PROP_KEY = /^[a-zA-Z][a-zA-Z0-9_]{0,63}$/
const STRING_KEY = /^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,63}$/
const ICON = /^[a-z0-9-]{1,48}$/
const HOST_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/
const VERSION_TEXT = /^\d+(?:\.\d+){0,2}(?:[-+][0-9A-Za-z.-]*)?$/
const COLOR = /^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/
const RESERVED_KEYS = new Set(['__proto__', 'constructor', 'prototype'])
const STRING_FORMATS = new Set(['text', 'textarea', 'url', 'color'])
// 这些关键字会改变 JSON Schema 的语义；子集不支持时必须明确报错，不能静默忽略。
const UNSUPPORTED_PROP_KEYWORDS = [
  '$ref',
  'oneOf',
  'anyOf',
  'allOf',
  'not',
  'if',
  'pattern',
  'properties',
  'patternProperties',
  'additionalProperties',
]
const MAX_PROPS = 32
const MAX_STRING_LENGTH = 4000
const MAX_ENUM = 32
const MAX_ARRAY_ITEMS = 50
const MAX_ARRAY_ITEM_LENGTH = 500
const MAX_I18N_STRINGS = 200

/**
 * @typedef {import('./custom-ui-manifest.mjs').ManifestError} ManifestError
 * @typedef {import('./custom-ui-manifest.mjs').PropField} PropField
 * @typedef {import('./custom-ui-manifest.mjs').PropsSchema} PropsSchema
 * @typedef {import('./custom-ui-manifest.mjs').NativeManifest} NativeManifest
 * @typedef {import('./custom-ui-manifest.mjs').ManifestPlacement} ManifestPlacement
 * @typedef {import('./custom-ui-manifest.mjs').ManifestCapabilities} ManifestCapabilities
 * @typedef {import('./custom-ui-manifest.mjs').ManifestLocaleText} ManifestLocaleText
 */

/** @param {unknown} value @returns {Record<string, unknown> | null} */
function record(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? /** @type {Record<string, unknown>} */ (value)
    : null
}

/** @param {Record<string, unknown>} value @param {string} key */
function own(value, key) {
  return Object.prototype.hasOwnProperty.call(value, key)
}

/**
 * 把孤立代理项换成 U+FFFD：Runtime 输出必须同时被 JSON.parse 与 serde_json 接受。
 * 不用 String.prototype.toWellFormed，前端构建目标是 safari16。
 * @param {string} value
 * @returns {string}
 */
export function toWellFormed(value) {
  let result = ''
  let changed = false
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index)
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = value.charCodeAt(index + 1)
      if (next >= 0xdc00 && next <= 0xdfff) {
        result += value[index] + value[index + 1]
        index += 1
        continue
      }
      result += '�'
      changed = true
      continue
    }
    if (code >= 0xdc00 && code <= 0xdfff) {
      result += '�'
      changed = true
      continue
    }
    result += value[index]
  }
  return changed ? result : value
}

/**
 * 组件目录内的安全相对路径：只用 / 分隔，每段是普通文件名（不以点开头），最多 8 段。
 * @param {unknown} value
 * @returns {value is string}
 */
export function isSafeComponentPath(value) {
  if (typeof value !== 'string' || !value || value.length > 512) return false
  const segments = value.split('/')
  return segments.length <= 8 && segments.every((segment) => FILE_SEGMENT.test(segment))
}

/**
 * 有 runtime: 'native' 的才是可运行组件；能解析成对象但没有这个字段的都按旧版（不再支持）。
 * @param {unknown} raw
 * @returns {'native' | 'legacy' | 'broken'}
 */
export function classifyManifest(raw) {
  const manifest = record(raw)
  if (!manifest) return 'broken'
  return manifest.runtime === 'native' ? 'native' : 'legacy'
}

/** @param {string} value @returns {number[] | null} */
function versionParts(value) {
  if (!VERSION_TEXT.test(value)) return null
  const core = value.split(/[-+]/)[0].split('.').map(Number)
  while (core.length < 3) core.push(0)
  return core
}

/**
 * @param {string} left
 * @param {string} right
 * @returns {number | null} 无法解析时返回 null
 */
export function compareHostVersions(left, right) {
  const a = versionParts(left)
  const b = versionParts(right)
  if (!a || !b) return null
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] < b[index] ? -1 : 1
  }
  return 0
}

/**
 * 主机名白名单项：小写、至少两段、可用一级 `*.` 通配；拒绝 IP 字面量。
 * @param {unknown} value
 * @returns {value is string}
 */
export function isAllowedNetworkHost(value) {
  if (typeof value !== 'string' || !value || value.length > 253) return false
  const host = value.startsWith('*.') ? value.slice(2) : value
  if (host.includes(':') || host.includes('*')) return false
  const labels = host.split('.')
  if (labels.length < 2 || !labels.every((label) => HOST_LABEL.test(label))) return false
  // 全数字的最后一段说明是 IPv4 字面量（含 1.2.3.4 与 0x 之外的十进制写法）。
  return !/^\d+$/.test(labels[labels.length - 1])
}

/**
 * @param {ManifestError[]} errors
 * @param {string} path
 * @param {string} code
 * @param {Record<string, string | number>} [params]
 */
function fail(errors, path, code, params) {
  errors.push(params ? { path, code, params } : { path, code })
}

/**
 * @param {unknown} value
 * @param {number} max
 * @returns {string | null}
 */
function boundedText(value, max) {
  if (typeof value !== 'string') return null
  const text = toWellFormed(value.trim())
  return text.length <= max ? text : null
}

/**
 * @param {unknown} raw
 * @param {string} path
 * @param {ManifestError[]} errors
 * @returns {Record<string, { title?: string, description?: string }>}
 */
function normalizeFieldI18n(raw, path, errors) {
  /** @type {Record<string, { title?: string, description?: string }>} */
  const result = {}
  if (raw === undefined) return result
  const source = record(raw)
  if (!source) {
    fail(errors, path, 'props_i18n_invalid')
    return result
  }
  for (const locale of MANIFEST_LOCALES) {
    if (!own(source, locale)) continue
    const entry = record(source[locale])
    if (!entry) {
      fail(errors, `${path}.${locale}`, 'props_i18n_invalid')
      continue
    }
    /** @type {{ title?: string, description?: string }} */
    const text = {}
    if (entry.title !== undefined) {
      const title = boundedText(entry.title, 80)
      if (title === null) fail(errors, `${path}.${locale}.title`, 'props_title_invalid')
      else text.title = title
    }
    if (entry.description !== undefined) {
      const description = boundedText(entry.description, 500)
      if (description === null)
        fail(errors, `${path}.${locale}.description`, 'props_description_invalid')
      else text.description = description
    }
    result[locale] = text
  }
  return result
}

/**
 * @param {PropField} field
 * @param {unknown} value
 * @param {string} path
 * @param {ManifestError[]} errors
 * @returns {unknown}
 */
function validateFieldValue(field, value, path, errors) {
  switch (field.type) {
    case 'string': {
      if (typeof value !== 'string') return fail(errors, path, 'props_type_invalid')
      const text = toWellFormed(value)
      if (text.length > field.maxLength)
        return fail(errors, path, 'props_too_long', { max: field.maxLength })
      if (field.enum && !field.enum.includes(text)) return fail(errors, path, 'props_enum_invalid')
      if (field.format === 'url' && text && !/^https?:\/\/[^\s]+$/i.test(text))
        return fail(errors, path, 'props_url_invalid')
      if (field.format === 'color' && text && !COLOR.test(text))
        return fail(errors, path, 'props_color_invalid')
      return text
    }
    case 'number':
    case 'integer': {
      if (typeof value !== 'number' || !Number.isFinite(value))
        return fail(errors, path, 'props_type_invalid')
      if (field.type === 'integer' && !Number.isInteger(value))
        return fail(errors, path, 'props_type_invalid')
      if (
        (field.minimum !== undefined && value < field.minimum) ||
        (field.maximum !== undefined && value > field.maximum)
      )
        return fail(errors, path, 'props_out_of_range', {
          ...(field.minimum !== undefined ? { min: field.minimum } : {}),
          ...(field.maximum !== undefined ? { max: field.maximum } : {}),
        })
      return value
    }
    case 'boolean':
      return typeof value === 'boolean' ? value : fail(errors, path, 'props_type_invalid')
    case 'array': {
      if (!Array.isArray(value)) return fail(errors, path, 'props_type_invalid')
      if (value.length > field.maxItems)
        return fail(errors, path, 'props_too_many', { max: field.maxItems })
      /** @type {string[]} */
      const items = []
      for (let index = 0; index < value.length; index += 1) {
        const item = value[index]
        if (typeof item !== 'string') return fail(errors, `${path}.${index}`, 'props_type_invalid')
        const text = toWellFormed(item)
        if (text.length > field.itemMaxLength)
          return fail(errors, `${path}.${index}`, 'props_too_long', { max: field.itemMaxLength })
        items.push(text)
      }
      return items
    }
  }
}

/**
 * @param {unknown} raw
 * @param {string} path
 * @param {ManifestError[]} errors
 * @returns {PropField | null}
 */
function normalizePropField(raw, path, errors) {
  const source = record(raw)
  if (!source) {
    fail(errors, path, 'props_field_invalid')
    return null
  }
  const before = errors.length
  for (const keyword of UNSUPPORTED_PROP_KEYWORDS) {
    if (own(source, keyword))
      fail(errors, `${path}.${keyword}`, 'props_keyword_unsupported', { keyword })
  }
  const type = source.type
  // 逐项拼出字段描述，最后整体当作 PropField 返回；中途的形状由上面的分支保证。
  /** @type {Record<string, any>} */
  let field
  if (type === 'string') {
    const format = source.format === undefined ? 'text' : source.format
    if (typeof format !== 'string' || !STRING_FORMATS.has(format))
      fail(errors, `${path}.format`, 'props_format_invalid')
    let maxLength = MAX_STRING_LENGTH
    if (source.maxLength !== undefined) {
      if (
        typeof source.maxLength !== 'number' ||
        !Number.isInteger(source.maxLength) ||
        source.maxLength < 1 ||
        source.maxLength > MAX_STRING_LENGTH
      )
        fail(errors, `${path}.maxLength`, 'props_max_length_invalid', { max: MAX_STRING_LENGTH })
      else maxLength = source.maxLength
    }
    field = { type: 'string', format, maxLength }
    if (source.enum !== undefined) {
      const values = source.enum
      if (
        !Array.isArray(values) ||
        !values.length ||
        values.length > MAX_ENUM ||
        values.some((item) => typeof item !== 'string' || !item || item.length > 200) ||
        new Set(values).size !== values.length
      )
        fail(errors, `${path}.enum`, 'props_enum_list_invalid', { max: MAX_ENUM })
      else field.enum = values.map((item) => toWellFormed(String(item)))
    }
  } else if (type === 'number' || type === 'integer') {
    field = { type }
    for (const name of /** @type {const} */ (['minimum', 'maximum', 'step'])) {
      if (source[name] === undefined) continue
      const value = source[name]
      if (
        typeof value !== 'number' ||
        !Number.isFinite(value) ||
        (name === 'step' && value <= 0) ||
        (type === 'integer' && !Number.isInteger(value))
      )
        fail(errors, `${path}.${name}`, 'props_number_invalid')
      else field[name] = value
    }
    if (
      typeof field.minimum === 'number' &&
      typeof field.maximum === 'number' &&
      field.minimum > field.maximum
    )
      fail(errors, `${path}.maximum`, 'props_number_invalid')
    if (source['x-unit'] !== undefined) {
      const unit = boundedText(source['x-unit'], 16)
      if (unit === null) fail(errors, `${path}.x-unit`, 'props_unit_invalid')
      else field.unit = unit
    }
  } else if (type === 'boolean') {
    field = { type: 'boolean' }
  } else if (type === 'array') {
    const items = record(source.items)
    if (
      !items ||
      items.type !== 'string' ||
      Object.keys(items).some((name) => name !== 'type' && name !== 'maxLength')
    )
      fail(errors, `${path}.items`, 'props_items_invalid')
    let itemMaxLength = MAX_ARRAY_ITEM_LENGTH
    if (items && items.maxLength !== undefined) {
      if (
        typeof items.maxLength !== 'number' ||
        !Number.isInteger(items.maxLength) ||
        items.maxLength < 1 ||
        items.maxLength > MAX_ARRAY_ITEM_LENGTH
      )
        fail(errors, `${path}.items.maxLength`, 'props_max_length_invalid', {
          max: MAX_ARRAY_ITEM_LENGTH,
        })
      else itemMaxLength = items.maxLength
    }
    let maxItems = MAX_ARRAY_ITEMS
    if (source.maxItems !== undefined) {
      if (
        typeof source.maxItems !== 'number' ||
        !Number.isInteger(source.maxItems) ||
        source.maxItems < 1 ||
        source.maxItems > MAX_ARRAY_ITEMS
      )
        fail(errors, `${path}.maxItems`, 'props_max_items_invalid', { max: MAX_ARRAY_ITEMS })
      else maxItems = source.maxItems
    }
    field = { type: 'array', maxItems, itemMaxLength }
  } else {
    fail(errors, `${path}.type`, 'props_type_unsupported')
    return null
  }
  if (source.title !== undefined) {
    const title = boundedText(source.title, 80)
    if (title === null) fail(errors, `${path}.title`, 'props_title_invalid')
    else field.title = title
  }
  if (source.description !== undefined) {
    const description = boundedText(source.description, 500)
    if (description === null) fail(errors, `${path}.description`, 'props_description_invalid')
    else field.description = description
  }
  const i18n = normalizeFieldI18n(source['x-i18n'], `${path}.x-i18n`, errors)
  if (Object.keys(i18n).length) field.i18n = i18n
  if (errors.length !== before) return null
  const normalized = /** @type {PropField} */ (/** @type {unknown} */ (field))
  if (source.default !== undefined) {
    const defaultErrors = /** @type {ManifestError[]} */ ([])
    const value = validateFieldValue(normalized, source.default, `${path}.default`, defaultErrors)
    if (defaultErrors.length) {
      errors.push(...defaultErrors.map((error) => ({ ...error, code: 'props_default_invalid' })))
      return null
    }
    normalized.default = /** @type {any} */ (value)
  }
  return normalized
}

/**
 * JSON Schema 子集：顶层 object，≤32 个属性；不认识的语义关键字明确报错。
 * @param {unknown} raw
 * @param {string} [basePath]
 * @returns {{ schema: PropsSchema | null, errors: ManifestError[] }}
 */
export function normalizePropsSchema(raw, basePath = 'props') {
  /** @type {ManifestError[]} */
  const errors = []
  /** @type {PropsSchema} */
  const schema = { type: 'object', properties: {}, required: [] }
  if (raw === undefined || raw === null) return { schema, errors }
  const source = record(raw)
  if (!source || (source.type !== undefined && source.type !== 'object')) {
    fail(errors, basePath, 'props_schema_invalid')
    return { schema: null, errors }
  }
  const properties = source.properties === undefined ? {} : record(source.properties)
  if (!properties) {
    fail(errors, `${basePath}.properties`, 'props_schema_invalid')
    return { schema: null, errors }
  }
  const keys = Object.keys(properties)
  if (keys.length > MAX_PROPS)
    fail(errors, `${basePath}.properties`, 'props_too_many_fields', { max: MAX_PROPS })
  for (const key of keys.slice(0, MAX_PROPS)) {
    const path = `${basePath}.properties.${key}`
    if (!PROP_KEY.test(key) || RESERVED_KEYS.has(key)) {
      fail(errors, path, 'props_key_invalid')
      continue
    }
    const field = normalizePropField(properties[key], path, errors)
    if (field) schema.properties[key] = field
  }
  if (source.required !== undefined) {
    const required = source.required
    if (
      !Array.isArray(required) ||
      required.some((key) => typeof key !== 'string' || !own(properties, key))
    )
      fail(errors, `${basePath}.required`, 'props_required_invalid')
    else schema.required = [...new Set(/** @type {string[]} */ (required))]
  }
  return errors.length ? { schema: null, errors } : { schema, errors }
}

/** @param {unknown} value */
function serializedBytes(value) {
  const text = JSON.stringify(value)
  // UTF-8 字节数：不依赖 TextEncoder 之外的环境 API，用代码点估算。
  let bytes = 0
  for (const char of text) {
    const code = /** @type {number} */ (char.codePointAt(0))
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4
  }
  return bytes
}

/**
 * 校验实例属性：拒绝未知字段、类型不符和超长；序列化后 ≤16 KiB。只保留用户给出的值，不回填默认值。
 * @param {PropsSchema | null | undefined} schema
 * @param {unknown} values
 * @returns {{ value: Record<string, unknown>, errors: ManifestError[] }}
 */
export function validateComponentProps(schema, values) {
  /** @type {ManifestError[]} */
  const errors = []
  /** @type {Record<string, unknown>} */
  const value = {}
  const properties = schema?.properties || {}
  if (values === undefined || values === null) values = {}
  const source = record(values)
  if (!source) {
    fail(errors, 'props', 'props_type_invalid')
    return { value, errors }
  }
  for (const key of Object.keys(source)) {
    const field = own(properties, key) ? properties[key] : undefined
    if (!field) {
      fail(errors, `props.${key}`, 'props_unknown')
      continue
    }
    if (source[key] === null || source[key] === undefined) continue
    const before = errors.length
    const normalized = validateFieldValue(field, source[key], `props.${key}`, errors)
    if (errors.length === before) value[key] = normalized
  }
  for (const key of schema?.required || []) {
    if (!own(value, key) && properties[key]?.default === undefined)
      fail(errors, `props.${key}`, 'props_required')
  }
  if (!errors.length && serializedBytes(value) > COMPONENT_PROPS_MAX_BYTES)
    fail(errors, 'props', 'props_too_large', { max: COMPONENT_PROPS_MAX_BYTES })
  return { value: errors.length ? {} : value, errors }
}

/**
 * @param {unknown} raw
 * @param {ManifestError[]} errors
 * @returns {ManifestPlacement[]}
 */
function normalizePlacements(raw, errors) {
  if (raw === undefined) {
    return COMPONENT_SURFACES.map((surface) => defaultPlacement(surface))
  }
  if (!Array.isArray(raw) || !raw.length || raw.length > COMPONENT_SURFACES.length) {
    fail(errors, 'placements', 'placements_invalid')
    return []
  }
  /** @type {ManifestPlacement[]} */
  const placements = []
  const seen = new Set()
  raw.forEach((item, index) => {
    const path = `placements.${index}`
    const source = typeof item === 'string' ? { surface: item } : record(item)
    if (!source || typeof source.surface !== 'string') {
      fail(errors, path, 'placement_invalid')
      return
    }
    const surface = /** @type {(typeof COMPONENT_SURFACES)[number]} */ (source.surface)
    if (!COMPONENT_SURFACES.includes(surface)) {
      fail(errors, `${path}.surface`, 'placement_surface_invalid', { surface: source.surface })
      return
    }
    if (seen.has(surface)) {
      fail(errors, `${path}.surface`, 'placement_duplicate', { surface })
      return
    }
    seen.add(surface)
    const placement = defaultPlacement(surface)
    if (surface === 'overview-card' && source.sizes !== undefined) {
      const sizes = source.sizes
      if (
        !Array.isArray(sizes) ||
        !sizes.length ||
        sizes.some((size) => !OVERVIEW_CARD_SIZES.includes(size)) ||
        new Set(sizes).size !== sizes.length
      )
        fail(errors, `${path}.sizes`, 'placement_sizes_invalid')
      else
        placement.sizes = OVERVIEW_CARD_SIZES.filter((size) =>
          /** @type {unknown[]} */ (sizes).includes(size),
        )
    }
    if (surface === 'floating' && source.size !== undefined) {
      const size = record(source.size)
      const limits = FLOATING_SIZE_LIMITS
      if (
        !size ||
        typeof size.w !== 'number' ||
        typeof size.h !== 'number' ||
        !Number.isInteger(size.w) ||
        !Number.isInteger(size.h) ||
        size.w < limits.minW ||
        size.w > limits.maxW ||
        size.h < limits.minH ||
        size.h > limits.maxH
      )
        fail(errors, `${path}.size`, 'placement_size_invalid', limits)
      else placement.size = { w: size.w, h: size.h }
    }
    placements.push(placement)
  })
  return placements
}

/** @param {(typeof COMPONENT_SURFACES)[number]} surface @returns {ManifestPlacement} */
function defaultPlacement(surface) {
  if (surface === 'overview-card') return { surface, sizes: [...OVERVIEW_CARD_SIZES] }
  if (surface === 'floating') return { surface, size: { ...FLOATING_DEFAULT_SIZE } }
  return { surface }
}

/**
 * capabilities 接受对象形式 { network: {...}, notify: true } 或数组形式 ['notify', { type: 'network', ... }]，
 * 统一规范成对象：每项都是对象，reason 缺省为空串。
 * @param {unknown} raw
 * @param {ManifestError[]} errors
 * @returns {ManifestCapabilities}
 */
function normalizeCapabilities(raw, errors) {
  /** @type {ManifestCapabilities} */
  const capabilities = {}
  if (raw === undefined) return capabilities
  /** @type {[string, unknown, string][]} */
  const entries = []
  if (Array.isArray(raw)) {
    raw.forEach((item, index) => {
      if (typeof item === 'string') entries.push([item, true, `capabilities.${index}`])
      else {
        const source = record(item)
        if (!source || typeof source.type !== 'string')
          fail(errors, `capabilities.${index}`, 'capability_invalid')
        else entries.push([source.type, source, `capabilities.${index}`])
      }
    })
  } else {
    const source = record(raw)
    if (!source) {
      fail(errors, 'capabilities', 'capability_invalid')
      return capabilities
    }
    for (const key of Object.keys(source)) entries.push([key, source[key], `capabilities.${key}`])
  }
  for (const [type, value, path] of entries) {
    if (!CAPABILITY_TYPES.includes(/** @type {any} */ (type))) {
      fail(errors, path, 'capability_unknown', { capability: type })
      continue
    }
    if (own(capabilities, type)) {
      fail(errors, path, 'capability_duplicate', { capability: type })
      continue
    }
    if (value === false) continue
    const source = value === true ? {} : record(value)
    if (!source) {
      fail(errors, path, 'capability_invalid', { capability: type })
      continue
    }
    let reason = ''
    if (source.reason !== undefined) {
      const text = boundedText(source.reason, 200)
      if (text === null) {
        fail(errors, `${path}.reason`, 'capability_reason_invalid')
        continue
      }
      reason = text
    }
    if (type === 'network') {
      const hosts = source.hosts
      if (!Array.isArray(hosts) || !hosts.length || hosts.length > 16) {
        fail(errors, `${path}.hosts`, 'network_hosts_invalid', { max: 16 })
        continue
      }
      const invalid = hosts.findIndex((host) => !isAllowedNetworkHost(host))
      if (invalid >= 0) {
        fail(errors, `${path}.hosts.${invalid}`, 'network_host_invalid')
        continue
      }
      capabilities.network = { hosts: [...new Set(/** @type {string[]} */ (hosts))], reason }
    } else if (type === 'storage') {
      let quotaKb = STORAGE_QUOTA_DEFAULT_KB
      if (source.quotaKb !== undefined) {
        const quota = source.quotaKb
        if (
          typeof quota !== 'number' ||
          !Number.isInteger(quota) ||
          quota < 1 ||
          quota > STORAGE_QUOTA_MAX_KB
        ) {
          fail(errors, `${path}.quotaKb`, 'storage_quota_invalid', { max: STORAGE_QUOTA_MAX_KB })
          continue
        }
        quotaKb = quota
      }
      capabilities.storage = { quotaKb, reason }
    } else {
      capabilities[/** @type {'notify' | 'sessions.read' | 'sessions.open'} */ (type)] = { reason }
    }
  }
  return capabilities
}

/**
 * @param {unknown} raw
 * @param {ManifestError[]} errors
 * @returns {Partial<Record<(typeof MANIFEST_LOCALES)[number], ManifestLocaleText>>}
 */
function normalizeManifestI18n(raw, errors) {
  /** @type {Partial<Record<(typeof MANIFEST_LOCALES)[number], ManifestLocaleText>>} */
  const result = {}
  if (raw === undefined) return result
  const source = record(raw)
  if (!source) {
    fail(errors, 'i18n', 'i18n_invalid')
    return result
  }
  // 其他语言先忽略（向前兼容），只认两种应用语言。
  for (const locale of MANIFEST_LOCALES) {
    if (!own(source, locale)) continue
    const path = `i18n.${locale}`
    const entry = record(source[locale])
    if (!entry) {
      fail(errors, path, 'i18n_invalid')
      continue
    }
    /** @type {ManifestLocaleText} */
    const text = {}
    if (entry.name !== undefined) {
      const name = boundedText(entry.name, 120)
      if (!name) fail(errors, `${path}.name`, 'name_invalid')
      else text.name = name
    }
    if (entry.description !== undefined) {
      const description = boundedText(entry.description, 500)
      if (description === null) fail(errors, `${path}.description`, 'description_invalid')
      else text.description = description
    }
    if (entry.strings !== undefined) {
      const strings = record(entry.strings)
      const keys = strings ? Object.keys(strings) : []
      if (!strings || keys.length > MAX_I18N_STRINGS) {
        fail(errors, `${path}.strings`, 'i18n_strings_invalid', { max: MAX_I18N_STRINGS })
      } else {
        /** @type {Record<string, string>} */
        const values = {}
        for (const key of keys) {
          const value = boundedText(strings[key], 500)
          if (!STRING_KEY.test(key) || RESERVED_KEYS.has(key) || value === null)
            fail(errors, `${path}.strings.${key}`, 'i18n_string_invalid')
          else values[key] = value
        }
        text.strings = values
      }
    }
    result[locale] = text
  }
  return result
}

/**
 * 规范化原生组件 manifest v2。任何错误都让 manifest 为 null（不能运行，也不能放置），错误逐项返回。
 * @param {string} id
 * @param {unknown} raw
 * @param {{ hostVersion?: string | null }} [options]
 * @returns {{ manifest: NativeManifest | null, errors: ManifestError[] }}
 */
export function normalizeNativeManifest(id, raw, { hostVersion = null } = {}) {
  /** @type {ManifestError[]} */
  const errors = []
  const source = record(raw)
  if (!source) {
    fail(errors, '', 'manifest_not_object')
    return { manifest: null, errors }
  }
  if (source.runtime !== 'native') fail(errors, 'runtime', 'runtime_not_native')

  const name = boundedText(source.name, 120)
  if (!name) fail(errors, 'name', 'name_invalid', { max: 120 })

  let version = '0.0.0'
  if (source.version !== undefined) {
    const text = boundedText(source.version, 64)
    if (!text) fail(errors, 'version', 'version_invalid', { max: 64 })
    else version = text
  }

  let description = ''
  if (source.description !== undefined) {
    const text = boundedText(source.description, 500)
    if (text === null) fail(errors, 'description', 'description_invalid', { max: 500 })
    else description = text
  }

  let entry = 'component.js'
  if (source.entry !== undefined) {
    if (
      !isSafeComponentPath(source.entry) ||
      !/\.m?js$/.test(source.entry) ||
      source.entry === 'manifest.json'
    )
      fail(errors, 'entry', 'entry_invalid')
    else entry = source.entry
  }

  /** @type {string[]} */
  let styles = []
  if (source.styles !== undefined) {
    const list = source.styles
    if (
      !Array.isArray(list) ||
      list.length > 4 ||
      list.some((item) => !isSafeComponentPath(item) || !item.endsWith('.css'))
    )
      fail(errors, 'styles', 'styles_invalid', { max: 4 })
    else styles = [...new Set(/** @type {string[]} */ (list))]
  }

  let icon = ''
  if (source.icon !== undefined) {
    if (typeof source.icon !== 'string' || !ICON.test(source.icon))
      fail(errors, 'icon', 'icon_invalid')
    else icon = source.icon
  }

  const i18n = normalizeManifestI18n(source.i18n, errors)
  const props = normalizePropsSchema(source.props)
  errors.push(...props.errors)
  const placements = normalizePlacements(source.placements, errors)
  const capabilities = normalizeCapabilities(source.capabilities, errors)

  let sdk = CUSTOM_UI_SDK_VERSION
  if (source.sdk !== undefined) {
    if (typeof source.sdk !== 'number' || !Number.isInteger(source.sdk) || source.sdk < 1)
      fail(errors, 'sdk', 'sdk_invalid')
    else if (source.sdk > CUSTOM_UI_SDK_VERSION)
      fail(errors, 'sdk', 'sdk_unsupported', { supported: CUSTOM_UI_SDK_VERSION })
    else sdk = source.sdk
  }

  let minHostVersion = ''
  if (source.minHostVersion !== undefined) {
    const text = typeof source.minHostVersion === 'string' ? source.minHostVersion.trim() : ''
    if (!text || text.length > 64 || !versionParts(text))
      fail(errors, 'minHostVersion', 'min_host_version_invalid')
    else {
      minHostVersion = text
      const order = hostVersion ? compareHostVersions(hostVersion, text) : null
      if (order !== null && order < 0)
        fail(errors, 'minHostVersion', 'host_too_old', {
          required: text,
          current: String(hostVersion),
        })
    }
  }

  if (errors.length || !name || !props.schema) return { manifest: null, errors }
  return {
    manifest: {
      runtime: 'native',
      id,
      name,
      version,
      description,
      entry,
      styles,
      icon,
      i18n,
      props: props.schema,
      placements,
      capabilities,
      sdk,
      minHostVersion,
    },
    errors,
  }
}

/**
 * @param {NativeManifest} manifest
 * @param {string} surface
 * @returns {ManifestPlacement | null}
 */
export function manifestPlacement(manifest, surface) {
  return manifest.placements.find((placement) => placement.surface === surface) || null
}
