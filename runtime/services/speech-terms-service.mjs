import { constants } from 'node:fs'
import { open, realpath } from 'node:fs/promises'
import { isAbsolute, join, relative, resolve, sep } from 'node:path'
import { parse as parseToml } from 'smol-toml'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

export const MAX_CUSTOM_SPEECH_TERMS = 64
export const MAX_SPEECH_TERM_LENGTH = 64
export const MAX_SPEECH_TERMS = 128
export const MAX_SPEECH_MANIFEST_BYTES = 256 * 1024
export const BUILTIN_SPEECH_TERMS = Object.freeze([
  'Pi Agent',
  'Pisper',
  'TypeScript',
  'JavaScript',
  'Python',
  'React',
  'useEffect',
  'useState',
  'Node.js',
  'npm install',
  'npm test',
  'cargo test',
  'Cargo',
  'Rust',
  'Tauri',
  'Vite',
  'Tailwind',
  'GitHub',
  'Git',
  'JSON',
  'TOML',
  'YAML',
  'API',
  'SDK',
  'MCP',
  'HTTP',
  'WebSocket',
  'Docker',
  'Kubernetes',
  'PostgreSQL',
])

const INVALID_TERM_CHARACTERS = /[^\p{L}\p{N} .+_-]/u
const SURROGATES = /[\uD800-\uDFFF]/
const DEPENDENCY_FIELDS = [
  'dependencies',
  'devDependencies',
  'peerDependencies',
  'optionalDependencies',
]
const CARGO_DEPENDENCY_FIELDS = ['dependencies', 'dev-dependencies', 'build-dependencies']

function normalizeTerm(value) {
  if (
    typeof value !== 'string' ||
    value.length > MAX_SPEECH_TERM_LENGTH ||
    SURROGATES.test(value) ||
    INVALID_TERM_CHARACTERS.test(value) ||
    !/[\p{L}\p{N}]/u.test(value)
  ) {
    throw new Error('语音词条须为 1 到 64 字符，仅允许文字、数字、空格、点、加号、连字符和下划线。')
  }
  return value.trim().replace(/ +/g, ' ')
}

function termKey(term) {
  // 包名的分词写法与原始驼峰写法归为同一词，保留优先来源的拼写。
  return term.toLowerCase().replace(/[ _-]/g, '')
}

function uniqueTerms(terms, limit) {
  const seen = new Set()
  const result = []
  for (const term of terms) {
    const key = termKey(term)
    if (seen.has(key)) continue
    seen.add(key)
    result.push(term)
    if (result.length === limit) break
  }
  return result
}

function validateUpdate(input) {
  if (!input || typeof input !== 'object' || Array.isArray(input)) {
    throw new Error('语音设置必须是 JSON 对象。')
  }
  if (Object.keys(input).some((key) => !['projectTermsEnabled', 'customTerms'].includes(key))) {
    throw new Error('语音设置包含未知字段。')
  }
  const result = {}
  if (Object.hasOwn(input, 'projectTermsEnabled')) {
    if (typeof input.projectTermsEnabled !== 'boolean') {
      throw new Error('projectTermsEnabled 必须是布尔值。')
    }
    result.projectTermsEnabled = input.projectTermsEnabled
  }
  if (Object.hasOwn(input, 'customTerms')) {
    if (!Array.isArray(input.customTerms) || input.customTerms.length > MAX_CUSTOM_SPEECH_TERMS) {
      throw new Error('自定义语音词条必须是数组，且不能超过 64 条。')
    }
    // 在去重前检查每一项，重复内容不能绕过数量或长度限制。
    const terms = Array.from(input.customTerms, normalizeTerm)
    result.customTerms = uniqueTerms(terms, MAX_CUSTOM_SPEECH_TERMS)
  }
  return result
}

function projectTerm(value) {
  if (typeof value !== 'string') return null
  const scoped = /^@[\p{L}\p{N}._-]+\/([\p{L}\p{N}._-]+)$/u.exec(value)
  if (scoped && scoped[0] !== value) return null
  const name = scoped ? scoped[1] : value
  try {
    normalizeTerm(name)
    return normalizeTerm(
      name
        .replace(/[-_]+/g, ' ')
        .replace(/([A-Z])([A-Z][a-z])/g, '$1 $2')
        .replace(/([a-z])([A-Z])/g, '$1 $2'),
    )
  } catch {
    return null
  }
}

function record(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

function dependencyNames(manifest, fields) {
  return fields.flatMap((field) => Object.keys(record(manifest[field])))
}

async function readManifest(root, filename, parse) {
  let handle
  try {
    const path = await realpath(join(root, filename))
    const inside = relative(root, path)
    if (!inside || isAbsolute(inside) || inside === '..' || inside.startsWith(`..${sep}`)) {
      return null
    }
    // 先约束真实路径；非阻塞打开避免恶意 FIFO 在检查文件类型前挂住请求。
    handle = await open(path, constants.O_RDONLY | (constants.O_NONBLOCK || 0))
    const info = await handle.stat()
    if (!info.isFile() || info.size > MAX_SPEECH_MANIFEST_BYTES) return null
    const bytes = Buffer.alloc(MAX_SPEECH_MANIFEST_BYTES)
    let length = 0
    while (length < bytes.length) {
      const { bytesRead } = await handle.read(bytes, length, bytes.length - length, length)
      if (!bytesRead) break
      length += bytesRead
    }
    if ((await handle.stat()).size > MAX_SPEECH_MANIFEST_BYTES) return null
    return record(
      parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes.subarray(0, length))),
    )
  } catch {
    // 项目词是可选增强，缺失、损坏或不可读的清单不能阻断语音输入。
    return null
  } finally {
    await handle?.close().catch(() => {})
  }
}

async function projectTerms(cwd) {
  if (typeof cwd !== 'string' || !cwd.trim()) return []
  let root
  try {
    root = await realpath(resolve(cwd))
  } catch {
    return []
  }
  const [npm, cargo] = await Promise.all([
    readManifest(root, 'package.json', JSON.parse),
    readManifest(root, 'Cargo.toml', parseToml),
  ])
  const names = []
  if (npm) {
    names.push(npm.name, ...dependencyNames(npm, DEPENDENCY_FIELDS))
  }
  if (cargo) {
    names.push(record(cargo.package).name, ...dependencyNames(cargo, CARGO_DEPENDENCY_FIELDS))
    names.push(...dependencyNames(record(cargo.workspace), CARGO_DEPENDENCY_FIELDS))
    for (const target of Object.values(record(cargo.target))) {
      names.push(...dependencyNames(record(target), CARGO_DEPENDENCY_FIELDS))
    }
  }
  return names.map(projectTerm).filter(Boolean)
}

export class SpeechTermsService {
  constructor({ dataDir }) {
    this.path = join(dataDir, 'speech-settings.json')
    this.write = Promise.resolve()
  }

  async readSettings() {
    return {
      projectTermsEnabled: true,
      customTerms: [],
      ...validateUpdate(await readJson(this.path, {})),
    }
  }

  async getSettings() {
    await this.write
    return { ...(await this.readSettings()), builtinTerms: [...BUILTIN_SPEECH_TERMS] }
  }

  async updateSettings(input) {
    const patch = validateUpdate(input)
    const operation = this.write.then(async () => {
      const settings = { ...(await this.readSettings()), ...patch }
      await writeJsonAtomic(this.path, settings)
      return { ...settings, builtinTerms: [...BUILTIN_SPEECH_TERMS] }
    })
    // 写失败只影响本次调用，后续合法更新仍可继续排队。
    this.write = operation.catch(() => {})
    return operation
  }

  async termsForWorkspace(cwd) {
    const settings = await this.getSettings()
    const project = settings.projectTermsEnabled ? await projectTerms(cwd) : []
    return uniqueTerms(
      [...BUILTIN_SPEECH_TERMS, ...settings.customTerms, ...project],
      MAX_SPEECH_TERMS,
    )
  }
}
