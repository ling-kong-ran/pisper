// 属性表单的纯逻辑（规格 5.3、计划 §4.4）：manifest 里已规范化的 props schema → 表单字段描述，
// 表单值 ↔ 实例属性，Runtime 返回的字段级错误 → 字段。不依赖 React，便于单测。
// 校验以 Runtime 为准（PATCH 时再校验一遍）：这里只做输入框文字到 JSON 值的换算，
// 换算不了的值原样交给 Runtime，由它按字段返回错误码，而不是在前端另写一套规则。
import type { PropField, PropsSchema } from '@shared/custom-ui-manifest.mjs'

// 不超过 4 项的 enum 用分段控件，超过用下拉（规格 5.3）。
export const ENUM_SEGMENTED_MAX = 4

type FieldBase = {
  key: string
  label: string
  description: string
  required: boolean
}

export type PropsFormField =
  | (FieldBase & {
      kind: 'text' | 'textarea' | 'url' | 'color'
      maxLength: number
      defaultValue?: string
    })
  | (FieldBase & { kind: 'segmented' | 'select'; options: string[]; defaultValue?: string })
  | (FieldBase & {
      kind: 'number'
      integer: boolean
      min?: number
      max?: number
      step?: number
      unit: string
      defaultValue?: number
    })
  | (FieldBase & { kind: 'switch'; defaultValue?: boolean })
  | (FieldBase & {
      kind: 'tags'
      maxItems: number
      itemMaxLength: number
      defaultValue?: string[]
    })

// 表单内部值：文字类与数字都是输入框里的字符串，开关是布尔，标签是字符串数组。
export type PropsFormValue = string | boolean | string[]
export type PropsFormValues = Record<string, PropsFormValue>

export type PropsFieldError = { code: string; params?: Record<string, string | number> }

// x-i18n（规范化后是 field.i18n）优先当前语言，再退回 title / 字段名。
function fieldText(field: PropField, key: string, language: string) {
  const localized = field.i18n?.[language as keyof NonNullable<PropField['i18n']>]
  return {
    label: localized?.title || field.title || key,
    description: localized?.description || field.description || '',
  }
}

/** schema → 字段描述，按 properties 的声明顺序。 */
export function buildPropsFormFields(
  schema: PropsSchema | null | undefined,
  language: string,
): PropsFormField[] {
  if (!schema || typeof schema.properties !== 'object' || !schema.properties) return []
  const required = new Set(Array.isArray(schema.required) ? schema.required : [])
  const fields: PropsFormField[] = []
  for (const [key, field] of Object.entries(schema.properties)) {
    if (!field || typeof field !== 'object') continue
    const base = { key, ...fieldText(field, key, language), required: required.has(key) }
    switch (field.type) {
      case 'string':
        if (Array.isArray(field.enum) && field.enum.length) {
          fields.push({
            ...base,
            kind: field.enum.length <= ENUM_SEGMENTED_MAX ? 'segmented' : 'select',
            options: [...field.enum],
            ...(field.default !== undefined ? { defaultValue: field.default } : {}),
          })
        } else {
          fields.push({
            ...base,
            kind:
              field.format === 'textarea' || field.format === 'url' || field.format === 'color'
                ? field.format
                : 'text',
            maxLength: field.maxLength,
            ...(field.default !== undefined ? { defaultValue: field.default } : {}),
          })
        }
        break
      case 'number':
      case 'integer':
        fields.push({
          ...base,
          kind: 'number',
          integer: field.type === 'integer',
          ...(field.minimum !== undefined ? { min: field.minimum } : {}),
          ...(field.maximum !== undefined ? { max: field.maximum } : {}),
          ...(field.step !== undefined ? { step: field.step } : {}),
          unit: field.unit || '',
          ...(field.default !== undefined ? { defaultValue: field.default } : {}),
        })
        break
      case 'boolean':
        fields.push({
          ...base,
          kind: 'switch',
          ...(field.default !== undefined ? { defaultValue: field.default } : {}),
        })
        break
      case 'array':
        fields.push({
          ...base,
          kind: 'tags',
          maxItems: field.maxItems,
          itemMaxLength: field.itemMaxLength,
          ...(field.default !== undefined ? { defaultValue: [...field.default] } : {}),
        })
        break
    }
  }
  return fields
}

/** 实例属性 → 表单值；没有保存过的字段显示默认值（与组件拿到的值一致：Runtime 不补默认值，组件自己处理缺省）。 */
export function propsToFormValues(
  fields: readonly PropsFormField[],
  props: Record<string, unknown> | null | undefined,
): PropsFormValues {
  const values: PropsFormValues = {}
  for (const field of fields) {
    const raw = props && Object.hasOwn(props, field.key) ? props[field.key] : field.defaultValue
    switch (field.kind) {
      case 'switch':
        values[field.key] = raw === true
        break
      case 'tags':
        values[field.key] = Array.isArray(raw)
          ? raw.filter((item): item is string => typeof item === 'string')
          : []
        break
      case 'number':
        values[field.key] = typeof raw === 'number' && Number.isFinite(raw) ? String(raw) : ''
        break
      default:
        values[field.key] = typeof raw === 'string' ? raw : ''
    }
  }
  return values
}

/**
 * 表单值 → 实例属性（PATCH 的 props）。空的可选文字 / 数字不写（组件按缺省处理）；
 * 数字框里不是数字的文字原样发出，由 Runtime 判为 props_type_invalid 并定位到这个字段。
 */
export function formValuesToProps(
  fields: readonly PropsFormField[],
  values: PropsFormValues,
): Record<string, unknown> {
  const props: Record<string, unknown> = {}
  for (const field of fields) {
    const value = values[field.key]
    switch (field.kind) {
      case 'switch':
        props[field.key] = value === true
        break
      case 'tags':
        if (Array.isArray(value) && value.length) props[field.key] = [...value]
        break
      case 'number': {
        const text = typeof value === 'string' ? value.trim() : ''
        if (!text) break
        const number = Number(text)
        props[field.key] = Number.isFinite(number) ? number : text
        break
      }
      case 'segmented':
      case 'select':
        if (typeof value === 'string' && value) props[field.key] = value
        break
      default:
        if (typeof value === 'string' && value) props[field.key] = value
    }
  }
  return props
}

/** 标签输入：一段文字按逗号 / 换行拆成标签，去空白、去重，接在已有标签后面。 */
export function addTags(current: readonly string[], input: string): string[] {
  const next = [...current]
  for (const part of input.split(/[,，\n]/)) {
    const tag = part.trim()
    if (tag && !next.includes(tag)) next.push(tag)
  }
  return next
}

/**
 * Runtime 的字段级错误（path 形如 props.repo 或 props.tags.2）→ 字段键；整体错误（path 为 props）放在 ''。
 * 每个字段只留第一条。
 */
export function mapPropsErrors(
  errors: readonly { path: string; code: string; params?: Record<string, string | number> }[],
): Record<string, PropsFieldError> {
  const result: Record<string, PropsFieldError> = {}
  for (const error of errors) {
    const match = /^props(?:\.([^.]+))?/.exec(error.path || '')
    const key = match ? (match[1] ?? '') : ''
    if (Object.hasOwn(result, key)) continue
    result[key] = error.params ? { code: error.code, params: error.params } : { code: error.code }
  }
  return result
}

/** 两份表单值是否相同（决定“保存”按钮是否可用）。 */
export function sameFormValues(left: PropsFormValues, right: PropsFormValues): boolean {
  const keys = new Set([...Object.keys(left), ...Object.keys(right)])
  for (const key of keys) {
    const a = left[key]
    const b = right[key]
    if (Array.isArray(a) || Array.isArray(b)) {
      if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false
      if (a.some((item, index) => item !== b[index])) return false
    } else if (a !== b) return false
  }
  return true
}
