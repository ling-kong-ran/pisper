// 执行环境注册表（B6）：本机之外，Bot 还可以在另一台运行“Pisper 执行节点”的电脑上干活。
// 这里只管有哪些节点、怎么配对、节点令牌对不对；节点在线与否看实时连接（executor-hub.mjs）。
// - 配对码一次性、10 分钟有效，只在本机界面生成；节点凭它换一个长期令牌，令牌只存摘要。
// - 节点自己上报允许访问的目录（roots）与 worktree 目录，并在节点一侧强制；这里只用于展示与预检。
import { createHash, randomBytes, randomInt, timingSafeEqual } from 'node:crypto'
import { isAbsolute } from 'node:path'
import { readJson, writeJsonAtomic } from '../../storage/json-file.mjs'

const STORE_VERSION = 1
export const MAX_ENVIRONMENTS = 16
const MAX_ROOTS = 16
const MAX_PATH_CHARS = 1024
const MAX_NAME_CHARS = 40
const PAIRING_TTL_MS = 10 * 60_000
const MAX_ACTIVE_CODES = 4
// 配对码猜错太多次时作废全部未用的配对码，防止暴力尝试。
const MAX_FAILED_ATTEMPTS = 20
const CODE_ALPHABET = 'ABCDEFGHJKMNPQRSTUVWXYZ23456789'
const ID_PATTERN = /^env_[A-Za-z0-9_-]{8,32}$/

export class EnvironmentError extends Error {
  constructor(code, message, statusCode = 400) {
    super(message)
    this.name = 'EnvironmentError'
    this.code = code
    this.statusCode = statusCode
  }
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function digest(token) {
  return createHash('sha256').update(String(token)).digest()
}

function cleanText(value, max) {
  return String(value ?? '')
    .replace(/[\u0000-\u001f\u007f]/g, ' ')
    .trim()
    .slice(0, max)
}

function normalizePath(value) {
  const path = String(value ?? '').trim()
  if (!path || path.length > MAX_PATH_CHARS || path.includes('\0') || !isAbsolute(path)) return ''
  return path
}

// 节点上报的目录与能力（不可信）：只留绝对路径与布尔值，数量有上限。
export function normalizeNodeReport(input) {
  const value = input && typeof input === 'object' ? input : {}
  const roots = [
    ...new Set((Array.isArray(value.roots) ? value.roots : []).map(normalizePath).filter(Boolean)),
  ].slice(0, MAX_ROOTS)
  const capabilities = value.capabilities && typeof value.capabilities === 'object'
  return {
    roots,
    worktreeRoot: normalizePath(value.worktreeRoot),
    capabilities: {
      shell: capabilities ? value.capabilities.shell === true : false,
      git: capabilities ? value.capabilities.git === true : false,
    },
    platform: cleanText(value.platform, 20),
  }
}

// 路径是否在某个允许目录之内（纯字符串比较，节点那边还会用真实路径再查一次）。
export function pathWithinRoots(path, roots) {
  const target = normalizePath(path)
  if (!target) return false
  return roots.some((root) => {
    const base = root.replace(/[\\/]+$/, '')
    return target === base || target.startsWith(`${base}/`) || target.startsWith(`${base}\\`)
  })
}

function publicEnvironment(environment) {
  const { tokenHash: _tokenHash, ...rest } = environment
  return clone(rest)
}

export class EnvironmentService {
  constructor({ path, now = () => Date.now() }) {
    this.path = path
    this.now = now
    this.environments = []
    this.codes = new Map()
    this.failedAttempts = 0
    this.writeQueue = Promise.resolve()
  }

  async init() {
    const stored = await readJson(this.path, { version: STORE_VERSION, environments: [] })
    this.environments = (Array.isArray(stored?.environments) ? stored.environments : [])
      .filter(
        (item) =>
          item &&
          ID_PATTERN.test(String(item.id)) &&
          typeof item.tokenHash === 'string' &&
          /^[0-9a-f]{64}$/.test(item.tokenHash),
      )
      .slice(0, MAX_ENVIRONMENTS)
      .map((item) => ({
        id: item.id,
        kind: 'node',
        name: cleanText(item.name, MAX_NAME_CHARS) || '执行节点',
        tokenHash: item.tokenHash,
        ...normalizeNodeReport(item),
        createdAt: String(item.createdAt || ''),
        lastSeenAt: item.lastSeenAt ? String(item.lastSeenAt) : null,
      }))
  }

  save() {
    const snapshot = { version: STORE_VERSION, environments: clone(this.environments) }
    this.writeQueue = this.writeQueue
      .catch(() => {})
      .then(() => writeJsonAtomic(this.path, snapshot, { mode: 0o600 }))
    return this.writeQueue
  }

  list() {
    return this.environments.map(publicEnvironment)
  }

  get(id) {
    const environment = this.environments.find((item) => item.id === id)
    return environment ? publicEnvironment(environment) : null
  }

  #pruneCodes() {
    const now = this.now()
    for (const [code, entry] of this.codes) if (entry.expiresAt <= now) this.codes.delete(code)
  }

  // 生成配对码（只在本机界面调用）。形如 ABCD-EF23，去掉了容易看错的字符。
  createPairing() {
    this.#pruneCodes()
    if (this.environments.length >= MAX_ENVIRONMENTS)
      throw new EnvironmentError(
        'environment_limit',
        `最多可以配对 ${MAX_ENVIRONMENTS} 台电脑。`,
        409,
      )
    while (this.codes.size >= MAX_ACTIVE_CODES) this.codes.delete(this.codes.keys().next().value)
    let raw = ''
    for (let index = 0; index < 8; index += 1) raw += CODE_ALPHABET[randomInt(CODE_ALPHABET.length)]
    const code = `${raw.slice(0, 4)}-${raw.slice(4)}`
    const expiresAt = this.now() + PAIRING_TTL_MS
    this.codes.set(code, { expiresAt })
    this.failedAttempts = 0
    return { code, expiresAt: new Date(expiresAt).toISOString() }
  }

  // 节点用配对码换令牌：码一次性，用过即删；令牌只返回这一次。
  async pair(input) {
    const value = input && typeof input === 'object' ? input : {}
    this.#pruneCodes()
    const code = String(value.code ?? '')
      .trim()
      .toUpperCase()
    if (!this.codes.has(code)) {
      this.failedAttempts += 1
      if (this.failedAttempts >= MAX_FAILED_ATTEMPTS) this.codes.clear()
      throw new EnvironmentError('pairing_code_invalid', '配对码无效或已过期。', 403)
    }
    this.codes.delete(code)
    if (this.environments.length >= MAX_ENVIRONMENTS)
      throw new EnvironmentError(
        'environment_limit',
        `最多可以配对 ${MAX_ENVIRONMENTS} 台电脑。`,
        409,
      )
    const report = normalizeNodeReport(value)
    if (!report.roots.length)
      throw new EnvironmentError('environment_roots_required', '执行节点至少要允许一个目录。')
    const token = randomBytes(32).toString('base64url')
    const at = new Date(this.now()).toISOString()
    const environment = {
      id: `env_${randomBytes(9).toString('base64url')}`,
      kind: 'node',
      name: cleanText(value.name, MAX_NAME_CHARS) || '执行节点',
      tokenHash: digest(token).toString('hex'),
      ...report,
      createdAt: at,
      lastSeenAt: at,
    }
    this.environments.push(environment)
    await this.save()
    return { environment: publicEnvironment(environment), token: `${environment.id}.${token}` }
  }

  // 节点请求的鉴权：“<id>.<令牌>”，按摘要常量时间比较。
  authenticate(bearer) {
    const text = String(bearer ?? '')
    const dot = text.indexOf('.')
    if (dot <= 0) return null
    const id = text.slice(0, dot)
    const environment = this.environments.find((item) => item.id === id)
    if (!environment) return null
    const expected = Buffer.from(environment.tokenHash, 'hex')
    const actual = digest(text.slice(dot + 1))
    return expected.length === actual.length && timingSafeEqual(expected, actual)
      ? publicEnvironment(environment)
      : null
  }

  // 节点每次连上来时上报目录与能力；只在有变化或距上次记录超过一分钟时落盘。
  async report(id, input) {
    const environment = this.environments.find((item) => item.id === id)
    if (!environment) return null
    const report = normalizeNodeReport(input)
    const changed = JSON.stringify(report) !== JSON.stringify(normalizeNodeReport(environment))
    const now = this.now()
    const stale = !environment.lastSeenAt || now - Date.parse(environment.lastSeenAt) > 60_000
    if (report.roots.length) Object.assign(environment, report)
    environment.lastSeenAt = new Date(now).toISOString()
    if (changed || stale) await this.save()
    return publicEnvironment(environment)
  }

  async remove(id, { inUseBy = [] } = {}) {
    const index = this.environments.findIndex((item) => item.id === id)
    if (index < 0) return false
    if (inUseBy.length)
      throw new EnvironmentError(
        'environment_in_use',
        `还有 Bot 在用这台电脑：${inUseBy.join('、')}。先给它们换一台电脑。`,
        409,
      )
    this.environments.splice(index, 1)
    await this.save()
    return true
  }
}
