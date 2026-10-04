// 表单和运行时共用输入边界，媒体值只携带资源引用，不接受 URL、本机路径或内联内容。
/** @typedef {import('./workflow-inputs.mjs').WorkflowInputDefinition} InputDefinition */
/** @typedef {import('./workflow-inputs.mjs').WorkflowInputValue} InputValue */
export const WORKFLOW_INPUT_MAX_LENGTH = 16_000
const RESERVED = new Set(['__proto__', 'constructor', 'prototype'])
const INPUT_NAME = /^[A-Za-z_][A-Za-z0-9_]{0,79}$/

export class WorkflowInputError extends Error {
  /** @param {string} code @param {string} [inputName] @param {string} [label] */
  constructor(code, inputName = '', label = '') {
    super(
      code === 'workflow_template_unknown'
        ? '工作流模板引用了未知或不可用的变量。'
        : '工作流输入无效，请检查字段名称、类型和必填值。',
    )
    this.code = code
    this.inputName = inputName
    this.label = label
    this.statusCode = 400
  }
}

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value))
    throw new WorkflowInputError('workflow_input_type')
  return /** @type {Record<string, unknown>} */ (value)
}

/** @param {unknown} value @returns {import('./workflow-inputs.mjs').WorkflowMedia} */
export function parseWorkflowMedia(value) {
  const media = record(value)
  if (
    Object.keys(media).some((key) => !['id', 'name', 'mimeType', 'size'].includes(key)) ||
    typeof media.id !== 'string' ||
    !/^[a-zA-Z0-9][a-zA-Z0-9_-]{0,79}$/.test(media.id) ||
    typeof media.name !== 'string' ||
    !media.name.trim() ||
    media.name.length > 160 ||
    /[\\/]/.test(media.name) ||
    typeof media.mimeType !== 'string' ||
    !['image/png', 'image/jpeg', 'image/webp', 'video/mp4', 'video/webm'].includes(
      media.mimeType,
    ) ||
    typeof media.size !== 'number' ||
    !Number.isSafeInteger(media.size) ||
    media.size <= 0 ||
    media.size > (media.mimeType.startsWith('image/') ? 8 : 64) * 1024 * 1024
  )
    throw new WorkflowInputError('workflow_input_type')
  return { id: media.id, name: media.name, mimeType: media.mimeType, size: media.size }
}

/** @param {InputDefinition} input @param {unknown} value @param {boolean} required @returns {InputValue} */
function normalizeValue(input, value, required) {
  const empty = value == null || (typeof value === 'string' && !value.trim())
  if (required && empty)
    throw new WorkflowInputError('workflow_input_required', input.name, input.label)
  if (empty)
    return input.type === 'boolean'
      ? false
      : input.type === 'image' || input.type === 'video'
        ? null
        : ''
  if (typeof value === 'string' && value.length > WORKFLOW_INPUT_MAX_LENGTH)
    throw new WorkflowInputError('workflow_input_too_long', input.name, input.label)
  if (input.type === 'number') {
    if (
      (typeof value !== 'number' && typeof value !== 'string') ||
      (typeof value === 'string' &&
        !/^[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:e[+-]?\d+)?$/i.test(value.trim())) ||
      !Number.isFinite(Number(value))
    )
      throw new WorkflowInputError('workflow_input_type', input.name, input.label)
    return Number(value)
  }
  if (input.type === 'boolean') {
    if (value !== true && value !== false && value !== 'true' && value !== 'false')
      throw new WorkflowInputError('workflow_input_type', input.name, input.label)
    return value === true || value === 'true'
  }
  if (input.type === 'image' || input.type === 'video') {
    const media = parseWorkflowMedia(value)
    if (!media.mimeType.startsWith(`${input.type}/`))
      throw new WorkflowInputError('workflow_input_type', input.name, input.label)
    return media
  }
  if (typeof value !== 'string')
    throw new WorkflowInputError('workflow_input_type', input.name, input.label)
  return value
}

/** @param {unknown} value @returns {InputDefinition[]} */
export function validateWorkflowInputDefinitions(value) {
  if (value === undefined) return []
  if (!Array.isArray(value) || value.length > 30)
    throw new WorkflowInputError('workflow_input_invalid_definition')
  const names = new Set()
  const ids = new Set()
  return value.map((raw, index) => {
    const input = record(raw)
    const name = input.name
    if (typeof name !== 'string' || !INPUT_NAME.test(name) || RESERVED.has(name))
      throw new WorkflowInputError('workflow_input_unsafe_name')
    const id = input.id === undefined ? `input-${index + 1}` : input.id
    const label = input.label === undefined ? name : input.label
    if (
      typeof id !== 'string' ||
      !id ||
      id.length > 80 ||
      typeof label !== 'string' ||
      !label.trim() ||
      label.length > 120 ||
      (input.description !== undefined &&
        (typeof input.description !== 'string' || input.description.length > 300)) ||
      (input.required !== undefined && typeof input.required !== 'boolean')
    )
      throw new WorkflowInputError('workflow_input_invalid_definition', name)
    if (names.has(name) || ids.has(id))
      throw new WorkflowInputError('workflow_input_duplicate_name', name, label)
    names.add(name)
    ids.add(id)
    const type = input.type === undefined ? 'string' : input.type
    if (
      type !== 'string' &&
      type !== 'text' &&
      type !== 'number' &&
      type !== 'boolean' &&
      type !== 'image' &&
      type !== 'video'
    )
      throw new WorkflowInputError('workflow_input_invalid_definition', name, label)
    /** @type {InputDefinition} */
    const definition = {
      id,
      name,
      label: label.trim(),
      type,
      required: input.required === true,
      defaultValue: '',
      description: typeof input.description === 'string' ? input.description : '',
    }
    // 空默认值保持未填写状态；必填校验属于每次运行，不阻止保存表单定义。
    definition.defaultValue =
      input.defaultValue == null || input.defaultValue === ''
        ? type === 'image' || type === 'video'
          ? null
          : ''
        : normalizeValue(definition, input.defaultValue, false)
    return definition
  })
}

/** @param {unknown} definitions @param {unknown} [supplied] @returns {Record<string, InputValue>} */
export function validateWorkflowInputs(definitions, supplied = {}) {
  const inputs = validateWorkflowInputDefinitions(definitions)
  const values = record(supplied)
  const effective = inputs.length
    ? inputs
    : [
        {
          id: 'task',
          name: 'task',
          label: 'Task',
          type: /** @type {const} */ ('text'),
          required: false,
          defaultValue: '',
          description: '',
        },
      ]
  const known = new Set(effective.map((input) => input.name))
  for (const key of Object.keys(values))
    if (!known.has(key) || RESERVED.has(key))
      throw new WorkflowInputError('workflow_input_unknown', key)
  /** @type {Record<string, InputValue>} */
  const result = {}
  for (const input of effective) {
    if (!inputs.length && !Object.hasOwn(values, input.name)) continue
    const value = Object.hasOwn(values, input.name) ? values[input.name] : input.defaultValue
    result[input.name] = normalizeValue(input, value, input.required)
  }
  return result
}

/** @param {string} template */
function variables(template) {
  return [...template.matchAll(/\{\{([^{}]*)\}\}/g)].map((match) => {
    const path = match[1].trim()
    const parts = path.split('.')
    if (!path || parts.some((part) => !/^[A-Za-z0-9_-]+$/.test(part) || RESERVED.has(part)))
      throw new WorkflowInputError('workflow_template_unknown', path.slice(0, 80))
    return { token: match[0], path, parts }
  })
}

/** @param {string} template @param {string[]} inputNames @param {string[]} nodeIds */
export function validateWorkflowTemplate(template, inputNames, nodeIds) {
  for (const { path, parts } of variables(template)) {
    const [root, field] = parts
    if (
      !['inputs', 'previous', 'nodes', 'workflow', 'run'].includes(root) ||
      (root === 'inputs' && field && !inputNames.includes(field)) ||
      (root === 'nodes' && (!field || !nodeIds.includes(field))) ||
      (root === 'workflow' && field && !['id', 'name', 'description'].includes(field)) ||
      (root === 'run' && field && !['id', 'startedAt'].includes(field))
    )
      throw new WorkflowInputError('workflow_template_unknown', path.slice(0, 80))
  }
}

/** @param {string} template @param {Record<string, unknown>} context */
export function renderWorkflowTemplate(template, context) {
  const replacements = new Map()
  for (const { token, path, parts } of variables(template)) {
    let value = /** @type {unknown} */ (context)
    for (const part of parts) {
      if (!value || typeof value !== 'object' || !Object.hasOwn(value, part))
        throw new WorkflowInputError('workflow_template_unknown', path.slice(0, 80))
      value = /** @type {Record<string, unknown>} */ (value)[part]
    }
    const text =
      value == null ? '' : typeof value === 'string' ? value : JSON.stringify(value, null, 2)
    replacements.set(token, text)
  }
  return template.replace(/\{\{([^{}]*)\}\}/g, (token) => replacements.get(token) || '')
}
