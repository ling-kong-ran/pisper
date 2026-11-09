// Pisper 的错误码：只有十来个通用码，具体是什么出错由 subject（少量名词）和 detail（英文补充）表达，
// 不再一条错误一句文案。Runtime 抛 PisperError，模型和日志看到英文 message；
// HTTP / SSE 带上 { code, params }，前端按界面语言用 errors 命名空间翻译，翻译不到时显示英文 message。
// 新增码或名词时同步 src/locales/{zh-CN,en-US}/errors.json（error-codes.test 守着）。

/**
 * 码 → [默认 HTTP 状态, 英文模板]；{subject} 缺省时用 request。
 * 状态沿用迁移前的习惯：未标注的错误一律 400，只有不存在 / 冲突 / 禁止 / 过大用专门状态，
 * 避免 5xx 被客户端当成 Runtime 掉线去重连。
 * @type {Readonly<Record<string, readonly [number, string]>>}
 */
export const ERROR_CODES = Object.freeze({
  invalid: [400, '{subject} is invalid.'],
  too_large: [413, '{subject} is too large.'],
  not_found: [404, '{subject} not found.'],
  exists: [409, '{subject} already exists.'],
  busy: [409, '{subject} is busy; wait for it to finish or stop it.'],
  limit: [400, '{subject} limit reached.'],
  unavailable: [400, '{subject} is unavailable.'],
  unsupported: [400, '{subject} is not supported here.'],
  forbidden: [403, '{subject} is not allowed.'],
  cancelled: [400, '{subject} was stopped.'],
  timeout: [400, '{subject} timed out.'],
  failed: [400, '{subject} failed.'],
})

/**
 * subject 名词 → 英文；只收常用的，没列到的直接显示原值。
 * @type {Readonly<Record<string, string>>}
 */
export const ERROR_SUBJECTS = Object.freeze({
  request: 'Request',
  input: 'Input',
  operation: 'Operation',
  session: 'Session',
  message: 'Message',
  attachment: 'Attachment',
  file: 'File',
  folder: 'Folder',
  path: 'Path',
  url: 'URL',
  model: 'Model',
  provider: 'Provider',
  api_key: 'API key',
  agent: 'Agent',
  bot: 'Bot',
  room: 'Room',
  skill: 'Skill',
  plugin: 'Plugin',
  tool: 'Tool',
  mcp: 'MCP server',
  channel: 'Channel',
  memory: 'Memory',
  schedule: 'Schedule',
  workflow: 'Workflow',
  workflow_run: 'Workflow run',
  task: 'Task',
  rule: 'Rule',
  machine: 'Machine',
  remote: 'Remote access',
  image: 'Image',
  speech: 'Speech',
  browser: 'Browser',
  device: 'Device',
  git: 'Git',
  svn: 'SVN',
  search: 'Web search',
  network: 'Network',
  login: 'Sign-in',
  config: 'Settings',
  component: 'Component',
  pet: 'Desktop pet',
  runtime: 'Runtime',
  capability: 'Feature',
})

const DEFAULT_SUBJECT = 'request'
const MAX_DETAIL_CHARS = 500

/** @param {unknown} [subject] */
export function errorSubjectLabel(subject) {
  const key = String(subject || DEFAULT_SUBJECT)
  return ERROR_SUBJECTS[key] ?? key
}

/**
 * 英文 message：模板 + 可选 detail（“…: detail”）。
 * @param {string} code
 * @param {{ subject?: unknown, detail?: unknown }} [params]
 */
export function errorMessage(code, params = {}) {
  const template = ERROR_CODES[code]?.[1] ?? ERROR_CODES.failed[1]
  const base = template.replace('{subject}', errorSubjectLabel(params.subject))
  const detail = String(params.detail ?? '').trim()
  return detail ? `${base.replace(/\.$/, '')}: ${detail}` : base
}

/**
 * @param {{ subject?: unknown, detail?: unknown } | undefined} params
 * @returns {{ subject?: string, detail?: string }}
 */
function cleanParams(params) {
  /** @type {{ subject?: string, detail?: string }} */
  const result = {}
  if (params?.subject) result.subject = String(params.subject)
  if (params?.detail != null && String(params.detail).trim())
    result.detail = String(params.detail).trim().slice(0, MAX_DETAIL_CHARS)
  return result
}

export class PisperError extends Error {
  /**
   * @param {string} code
   * @param {{ subject?: unknown, detail?: unknown }} [params]
   * @param {{ status?: number, cause?: unknown }} [options]
   */
  constructor(code, params = {}, options = {}) {
    const known = Object.hasOwn(ERROR_CODES, code) ? code : 'failed'
    const clean = cleanParams(params)
    super(errorMessage(known, clean), options.cause === undefined ? {} : { cause: options.cause })
    this.name = 'PisperError'
    this.code = known
    this.params = clean
    this.statusCode = options.status ?? ERROR_CODES[known][0]
  }
}

/**
 * 简写：throw pisperError('not_found', 'workflow') / pisperError('invalid', 'model', 'use provider/modelId')
 * @param {string} code
 * @param {string} [subject]
 * @param {unknown} [detail]
 * @param {{ status?: number, cause?: unknown }} [options]
 */
export function pisperError(code, subject, detail, options) {
  return new PisperError(code, { subject, detail }, options)
}

/**
 * @param {unknown} error
 * @returns {error is PisperError}
 */
export function isPisperError(error) {
  return error instanceof PisperError
}

/** 给 HTTP / SSE 的错误字段：PisperError 带 code 与 params，其它错误为空对象。 */
/** @param {unknown} error */
export function errorPayload(error) {
  return isPisperError(error) ? { code: error.code, params: error.params } : {}
}
