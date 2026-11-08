// 持久权限规则：在“每次精确记住同一条命令”之外，按前缀/目录批量放行或禁止。
// - 允许规则：bash 命令前缀，或文件写入（edit/write）的工作区相对目录；只替代普通审批，
//   命令守卫标出的危险命令、密码框输入等强制确认仍然要用户确认。
// - 禁止规则：bash 命令前缀；任何执行模式下都拦截（包括完全访问），用户明确说过不许的就不执行。
// - 范围：全局，或某个项目目录（规范化后的真实路径）。
// 允许规则只匹配不含 Shell 控制符的单条命令，避免 `npm test && rm -rf ~` 借着 `npm test` 的规则通过；
// 禁止规则按控制符拆开逐段匹配，宁可多拦。
import { randomUUID } from 'node:crypto'
import { existsSync, realpathSync } from 'node:fs'
import { isAbsolute, relative, resolve, sep } from 'node:path'
import { pisperError } from '../../shared/errors.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

export const MAX_PERMISSION_RULES = 200
const MAX_PATTERN_CHARS = 200
const SHELL_CONTROL_PATTERN = /[;&|`$<>\n\r\\(){}]/
const SEGMENT_SPLIT_PATTERN = /&&|\|\||[;&|\n\r]|\$\(|`/

function canonicalDirectory(input) {
  const path = resolve(input)
  try {
    return existsSync(path) ? realpathSync.native(path) : path
  } catch {
    return path
  }
}

function prefixMatches(command, prefix) {
  return command === prefix || command.startsWith(`${prefix} `)
}

function normalizeCommand(value) {
  return String(value || '')
    .trim()
    .replace(/[ \t]+/g, ' ')
}

/** @param {unknown} input */
export function normalizePermissionRule(input) {
  const value = input && typeof input === 'object' ? input : {}
  const effect = value.effect === 'deny' ? 'deny' : value.effect === 'allow' ? 'allow' : ''
  if (!effect) throw pisperError('invalid', 'rule', 'effect: allow or deny')
  const tool = value.tool === 'bash' || value.tool === 'files' ? value.tool : ''
  if (!tool) throw pisperError('invalid', 'rule', 'tool: bash or files')
  if (tool === 'files' && effect === 'deny')
    throw pisperError('invalid', 'rule', 'files rules can only allow')
  let pattern = typeof value.pattern === 'string' ? value.pattern.trim() : ''
  if (!pattern) throw pisperError('invalid', 'rule', 'pattern is required')
  if (pattern.length > MAX_PATTERN_CHARS)
    throw pisperError('too_large', 'rule', `pattern max ${MAX_PATTERN_CHARS} characters`)
  if (tool === 'bash') {
    pattern = normalizeCommand(pattern)
    if (effect === 'allow' && SHELL_CONTROL_PATTERN.test(pattern))
      throw pisperError(
        'invalid',
        'rule',
        'allow needs a single command prefix without pipes, redirects or variables',
      )
  } else {
    pattern = pattern
      .replaceAll('\\', '/')
      .replace(/^\.\/+/, '')
      .replace(/\/+$/, '')
    if (!pattern || pattern === '.' || isAbsolute(pattern) || /^[a-z]:/i.test(pattern))
      throw pisperError(
        'invalid',
        'rule',
        'files pattern must be a relative folder in the workspace',
      )
    if (pattern.split('/').some((part) => part === '..' || part === ''))
      throw pisperError(
        'invalid',
        'rule',
        'files pattern must be a relative folder in the workspace',
      )
  }
  const cwd = typeof value.cwd === 'string' && value.cwd.trim() ? value.cwd.trim() : ''
  if (cwd && !isAbsolute(cwd)) throw pisperError('invalid', 'rule', 'cwd must be absolute')
  return { effect, tool, pattern, cwd: cwd ? canonicalDirectory(cwd) : '' }
}

export class PermissionRuleService {
  constructor({ path }) {
    this.path = path
    this.rules = null
    this.write = Promise.resolve()
  }

  async load() {
    if (this.rules) return this.rules
    const data = await readJson(this.path, { version: 1, rules: [] }).catch(() => null)
    const rules = []
    for (const item of Array.isArray(data?.rules) ? data.rules : []) {
      try {
        rules.push({
          id: typeof item.id === 'string' && item.id ? item.id : randomUUID(),
          ...normalizePermissionRule(item),
          createdAt: typeof item.createdAt === 'string' ? item.createdAt : '',
        })
      } catch {
        // 手改坏的规则直接忽略，不让一条坏规则拖垮全部审批。
      }
    }
    this.rules = rules
    return rules
  }

  async list() {
    return { rules: [...(await this.load())] }
  }

  async add(input) {
    const rule = normalizePermissionRule(input)
    return this.mutate((rules) => {
      const existing = rules.find(
        (item) =>
          item.effect === rule.effect &&
          item.tool === rule.tool &&
          item.pattern === rule.pattern &&
          item.cwd === rule.cwd,
      )
      if (existing) return rules
      if (rules.length >= MAX_PERMISSION_RULES)
        throw pisperError('limit', 'rule', `max ${MAX_PERMISSION_RULES}`)
      return [...rules, { id: randomUUID(), ...rule, createdAt: new Date().toISOString() }]
    })
  }

  async remove(id) {
    return this.mutate((rules) => rules.filter((rule) => rule.id !== String(id || '')))
  }

  mutate(change) {
    this.write = this.write
      .catch(() => {})
      .then(async () => {
        const next = change(await this.load())
        if (next !== this.rules) {
          await writeJsonAtomic(this.path, { version: 1, rules: next })
          this.rules = next
        }
        return { rules: [...next] }
      })
    return this.write
  }

  // 返回命中的规则（禁止优先），没有命中返回 null。
  async evaluate({ cwd, toolName, args }) {
    const rules = await this.load()
    if (!rules.length) return null
    const root = canonicalDirectory(cwd)
    const scoped = rules.filter((rule) => !rule.cwd || rule.cwd === root)
    if (toolName === 'bash') {
      const command = normalizeCommand(args?.command)
      if (!command) return null
      const segments = command
        .split(SEGMENT_SPLIT_PATTERN)
        .map((part) => normalizeCommand(part.replace(/^[({\s]+/, '')))
        .filter(Boolean)
      const deny = scoped.find(
        (rule) =>
          rule.tool === 'bash' &&
          rule.effect === 'deny' &&
          segments.some((segment) => prefixMatches(segment, rule.pattern)),
      )
      if (deny) return deny
      if (SHELL_CONTROL_PATTERN.test(command)) return null
      return (
        scoped.find(
          (rule) =>
            rule.tool === 'bash' && rule.effect === 'allow' && prefixMatches(command, rule.pattern),
        ) || null
      )
    }
    if (toolName === 'edit' || toolName === 'write') {
      const raw = typeof args?.path === 'string' ? args.path.trim() : ''
      if (!raw) return null
      const target = relative(root, resolve(root, raw)).split(sep).join('/')
      if (!target || target === '..' || target.startsWith('../') || isAbsolute(target)) return null
      return (
        scoped.find(
          (rule) =>
            rule.tool === 'files' &&
            rule.effect === 'allow' &&
            (target === rule.pattern || target.startsWith(`${rule.pattern}/`)),
        ) || null
      )
    }
    return null
  }
}
