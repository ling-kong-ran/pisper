// 工作台有自己的项目和任务契约；共享图片引用与像素结果，不保存工作流实体或节点。
import { parseWorkflowMedia } from '../workflow/workflow-inputs.mjs'
import { normalizeImageFrameEdits } from '../image/image-frame-edits.mjs'
import { IMAGE_DIRECTIONS, parseImageOutput } from '../image/image-operations.mjs'

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i
const PROJECT_FIELDS = [
  'id',
  'name',
  'prompt',
  'reference',
  'originalReference',
  'frameCount',
  'directions',
  'model',
  'actions',
]
const STATUSES = ['running', 'completed', 'failed', 'cancelled', 'interrupted']

export class GameAssetError extends Error {
  /** @param {string} code @param {number} [statusCode] */
  constructor(code, statusCode = 400) {
    super(code)
    this.code = code
    this.statusCode = statusCode
  }
}

/** @returns {never} */
function invalid() {
  throw new GameAssetError('game_assets_invalid')
}
/** @param {unknown} value @param {string[]} fields @returns {Record<string, unknown>} */
function record(value, fields) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return invalid()
  if (Object.keys(value).some((key) => !fields.includes(key))) return invalid()
  return /** @type {Record<string, unknown>} */ (value)
}
/** @param {unknown} value @param {number} maximum @param {boolean} [required] */
function text(value, maximum, required = false) {
  if (typeof value !== 'string' || value.length > maximum || (required && !value.trim()))
    return invalid()
  return value.trim()
}
/** @param {unknown} value */
function id(value) {
  if (typeof value !== 'string' || !UUID.test(value)) return invalid()
  return value
}
/** @param {unknown} value */
function date(value) {
  if (
    typeof value !== 'string' ||
    !Number.isFinite(Date.parse(value)) ||
    new Date(value).toISOString() !== value
  )
    return invalid()
  return value
}
/** @param {unknown} value @param {number} min @param {number} max */
function integer(value, min, max) {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < min || value > max)
    return invalid()
  return value
}
/** @param {unknown} value */
function image(value) {
  if (value === null || value === undefined) return null
  try {
    const media = parseWorkflowMedia(value)
    if (!media.mimeType.startsWith('image/')) return invalid()
    return media
  } catch {
    return invalid()
  }
}

/** @param {unknown} value @returns {import('./game-assets.mjs').GameAssetProjectInput} */
export function parseGameAssetProjectInput(value) {
  const input = record(value, PROJECT_FIELDS)
  const directions = input.directions ?? [...IMAGE_DIRECTIONS]
  const actions = input.actions ?? []
  if (
    !Array.isArray(directions) ||
    !directions.length ||
    directions.length > 8 ||
    directions.some((item) => !IMAGE_DIRECTIONS.some((direction) => direction === item)) ||
    new Set(directions).size !== directions.length ||
    !Array.isArray(actions) ||
    actions.length > 8
  )
    return invalid()
  const frameCount = integer(input.frameCount === undefined ? 4 : input.frameCount, 1, 16)
  const parsedActions = actions.map((value) => {
    const action = record(value, ['id', 'name', 'prompt', 'enabled'])
    if (
      typeof action.id !== 'string' ||
      !/^[A-Za-z0-9][A-Za-z0-9_-]{0,39}$/.test(action.id) ||
      typeof action.enabled !== 'boolean'
    )
      return invalid()
    return {
      id: action.id,
      name: text(action.name, 100, true),
      prompt: text(action.prompt ?? '', 2000),
      enabled: action.enabled,
    }
  })
  if (
    new Set(parsedActions.map((action) => action.id)).size !== parsedActions.length ||
    parsedActions.filter((action) => action.enabled).length * directions.length * frameCount > 512
  )
    return invalid()
  let model = null
  if (input.model !== null && input.model !== undefined) {
    const selected = record(input.model, ['provider', 'model'])
    model = { provider: text(selected.provider, 200, true), model: text(selected.model, 300, true) }
  }
  return {
    ...(input.id === undefined ? {} : { id: id(input.id) }),
    name: text(input.name, 100, true),
    prompt: text(input.prompt ?? '', 8000),
    reference: image(input.reference),
    originalReference: image(input.originalReference),
    frameCount,
    directions: /** @type {import('../image/image-operations.mjs').ImageDirection[]} */ ([
      ...directions,
    ]),
    model,
    actions: parsedActions,
  }
}

/** @param {unknown} value @returns {import('./game-assets.mjs').GameAssetProject} */
export function parseGameAssetProject(value) {
  const stored = record(value, [...PROJECT_FIELDS, 'createdAt', 'updatedAt'])
  const { createdAt, updatedAt, ...input } = stored
  return {
    ...parseGameAssetProjectInput(input),
    id: id(input.id),
    createdAt: date(createdAt),
    updatedAt: date(updatedAt),
  }
}

/** @param {unknown} value @returns {import('./game-assets.mjs').GameAssetJob} */
export function parseGameAssetJob(value) {
  const stored = record(value, [
    'id',
    'projectId',
    'status',
    'startedAt',
    'finishedAt',
    'completed',
    'total',
    'error',
    'output',
    'originalOutput',
    'edits',
    'revision',
  ])
  if (
    typeof stored.status !== 'string' ||
    !STATUSES.includes(stored.status) ||
    (stored.error !== null &&
      (typeof stored.error !== 'string' ||
        !/^(?:game_assets|workflow_image|workflow_media|sprite_engine)_[a-z_]{1,60}$/.test(
          stored.error,
        )))
  )
    return invalid()
  const total = integer(stored.total, 1, 8)
  const completed = integer(stored.completed, 0, total)
  if (
    (stored.status === 'running') !== (stored.finishedAt === null) ||
    (stored.status === 'completed' && completed !== total)
  )
    return invalid()
  /** @type {import('../image/image-operations.mjs').ImageOutput} */
  let output
  /** @type {import('../image/image-operations.mjs').ImageOutput} */
  let originalOutput
  /** @type {import('../image/image-frame-edits.mjs').ImageFrameEdits|undefined} */
  let edits
  try {
    output = parseImageOutput(stored.output)
    originalOutput = parseImageOutput(stored.originalOutput)
    if (stored.edits !== undefined) {
      edits = normalizeImageFrameEdits(stored.edits)
      if (edits.frames.some((frame) => frame.sourceIndex >= originalOutput.frames.length))
        return invalid()
      if (edits.frames.length !== output.frames.length) return invalid()
    }
  } catch {
    return invalid()
  }
  return {
    id: id(stored.id),
    projectId: id(stored.projectId),
    status: /** @type {import('./game-assets.mjs').GameAssetJob['status']} */ (stored.status),
    startedAt: date(stored.startedAt),
    finishedAt: stored.finishedAt === null ? null : date(stored.finishedAt),
    completed,
    total,
    error: /** @type {string|null} */ (stored.error),
    output,
    originalOutput,
    revision: integer(stored.revision, 0, Number.MAX_SAFE_INTEGER),
    ...(edits ? { edits } : {}),
  }
}

/** @param {unknown} value @returns {import('./game-assets.mjs').GameAssetsCatalog} */
export function parseGameAssetsCatalog(value) {
  const stored = record(value, ['projects', 'jobs'])
  if (
    !Array.isArray(stored.projects) ||
    stored.projects.length > 100 ||
    !Array.isArray(stored.jobs) ||
    stored.jobs.length > 100
  )
    return invalid()
  const projects = stored.projects.map(parseGameAssetProject)
  const jobs = stored.jobs.map(parseGameAssetJob)
  if (
    new Set(projects.map((project) => project.id)).size !== projects.length ||
    new Set(jobs.map((job) => job.id)).size !== jobs.length ||
    jobs.some((job) => !projects.some((project) => project.id === job.projectId))
  )
    return invalid()
  return { projects, jobs }
}
