import { createHash } from 'node:crypto'
import { existsSync, statSync } from 'node:fs'
import { cp, lstat, mkdir, readFile, readdir, rm } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, dirname, extname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import {
  DefaultPackageManager,
  DefaultResourceLoader,
  loadSkills,
} from '@earendil-works/pi-coding-agent'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const SKILLS_STATE_VERSION = 2
const MAX_SKILL_SOURCE_CHARS = 2_000
const MAX_SKILLS_PER_INSTALL = 100
const MAX_SKILL_FILES = 10_000
const MAX_SKILL_BYTES = 256 * 1024 * 1024
const DASHBOARD_CACHE_TTL_MS = 3_000

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function normalizedPath(value) {
  const path = resolve(String(value || ''))
  return process.platform === 'win32' ? path.toLowerCase() : path
}

function skillId(filePath) {
  return createHash('sha256').update(normalizedPath(filePath)).digest('hex').slice(0, 20)
}

function slug(value) {
  const result = String(value || '')
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, '-')
    .replace(/-{2,}/g, '-')
    .replace(/^-|-$/g, '')
  return result || 'skill'
}

function normalizeState(input) {
  const overrides = input && typeof input === 'object' && input.overrides && typeof input.overrides === 'object'
    ? input.overrides
    : {}
  const installed = input && typeof input === 'object' && input.installed && typeof input.installed === 'object'
    ? input.installed
    : {}
  const normalizedOverrides = {}
  const normalizedInstalled = {}
  for (const [path, value] of Object.entries(overrides)) {
    if (!value || typeof value !== 'object') continue
    const item = {}
    if (typeof value.enabled === 'boolean') item.enabled = value.enabled
    if (typeof value.modelInvocation === 'boolean') item.modelInvocation = value.modelInvocation
    if (Object.keys(item).length) normalizedOverrides[normalizedPath(path)] = item
  }
  for (const [path, value] of Object.entries(installed)) {
    if (!value || typeof value !== 'object') continue
    normalizedInstalled[normalizedPath(path)] = {
      source: safeSourceLabel(value.source),
      installedAt: String(value.installedAt || ''),
    }
  }
  return { version: SKILLS_STATE_VERSION, overrides: normalizedOverrides, installed: normalizedInstalled }
}

function expandPath(value, cwd) {
  const input = String(value || '').trim()
  if (!input) return ''
  if (input === '~') return homedir()
  if (input.startsWith(`~${sep}`) || input.startsWith('~/') || input.startsWith('~\\')) return join(homedir(), input.slice(2))
  return isAbsolute(input) ? resolve(input) : resolve(cwd, input)
}

async function pathExists(path) {
  try {
    return await lstat(path)
  } catch (error) {
    if (error?.code === 'ENOENT') return null
    throw error
  }
}

function pathInside(root, target) {
  const result = relative(resolve(root), resolve(target))
  return result === '' || (!result.startsWith(`..${sep}`) && result !== '..' && !isAbsolute(result))
}

async function validateSkillSource(path) {
  const pending = [path]
  let entries = 0
  let files = 0
  let bytes = 0
  while (pending.length) {
    const current = pending.pop()
    const info = await lstat(current)
    if (info.isSymbolicLink()) throw new Error('技能来源包含符号链接，出于安全原因无法安装。')
    if (info.isDirectory()) {
      const children = await readdir(current)
      entries += children.length
      if (entries > MAX_SKILL_FILES) throw new Error(`单个技能最多包含 ${MAX_SKILL_FILES} 个文件和目录。`)
      for (const entry of children) pending.push(join(current, entry))
      continue
    }
    if (!info.isFile()) throw new Error('技能来源包含不支持的特殊文件。')
    files += 1
    bytes += info.size
    if (files > MAX_SKILL_FILES) throw new Error(`单个技能最多包含 ${MAX_SKILL_FILES} 个文件。`)
    if (bytes > MAX_SKILL_BYTES) throw new Error('单个技能大小不能超过 256 MB。')
  }
}

function parseFrontmatterDetails(content) {
  const block = String(content || '').match(/^---\s*\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/)?.[1] || ''
  const line = (name) => block.match(new RegExp(`^\\s*${name}\\s*:\\s*(.+?)\\s*$`, 'mi'))?.[1]?.replace(/^['"]|['"]$/g, '') || ''
  const allowedTools = line('allowed-tools')
    .replace(/^\[|\]$/g, '')
    .split(/[\s,]+/)
    .map((item) => item.replace(/^['"]|['"]$/g, '').trim())
    .filter(Boolean)
  return {
    version: line('version') || line('compatibility') || 'latest',
    license: line('license'),
    allowedTools,
  }
}

function sourceLabel(skill) {
  const info = skill.sourceInfo || {}
  if (info.origin === 'package') return info.source || 'package'
  if (info.scope === 'project') return 'project'
  if (info.scope === 'user') return 'user'
  return info.source || 'custom'
}

function safeSourceLabel(value) {
  const source = String(value || '').trim()
  if (!/^(?:git\+)?https?:\/\//i.test(source)) return source.slice(0, MAX_SKILL_SOURCE_CHARS)
  try {
    const url = new URL(source.replace(/^git\+/, ''))
    if (url.username) url.username = '***'
    if (url.password) url.password = '***'
    for (const key of [...url.searchParams.keys()]) {
      if (/token|key|secret|password|auth/i.test(key)) url.searchParams.set(key, '***')
    }
    return url.toString().slice(0, MAX_SKILL_SOURCE_CHARS)
  } catch {
    return source.replace(/:\/\/[^/@\s]+@/, '://***@').slice(0, MAX_SKILL_SOURCE_CHARS)
  }
}

function mapSkillResourcePath(resource) {
  const path = String(resource?.path || '')
  if (!path) return ''
  const metadata = resource?.metadata || {}
  if (metadata.source !== 'auto' && metadata.origin !== 'package') return path
  try {
    const stats = statSync(path)
    if (!stats.isDirectory()) return path
  } catch {
    return path
  }
  const skillFile = join(path, 'SKILL.md')
  return existsSync(skillFile) ? skillFile : path
}

export class SkillsService {
  constructor({ path, agentDir, cwd, getSettingsManager, createPackageManager, extensionFactories = [] } = {}) {
    this.path = path
    this.agentDir = agentDir
    this.cwd = cwd || process.cwd()
    this.skillsDir = join(agentDir, 'skills')
    this.getSettingsManager = getSettingsManager || (() => null)
    this.createPackageManager = createPackageManager || ((options) => new DefaultPackageManager(options))
    this.extensionFactories = extensionFactories
    this.state = { version: SKILLS_STATE_VERSION, overrides: {}, installed: {} }
    this.write = Promise.resolve()
    this.dashboardCache = null
    this.dashboardInflight = new Map()
  }

  async init() {
    await mkdir(this.skillsDir, { recursive: true })
    this.state = normalizeState(await readJson(this.path, { version: SKILLS_STATE_VERSION, overrides: {}, installed: {} }))
  }

  save() {
    const snapshot = clone(this.state)
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  overrideFor(skill) {
    return this.state.overrides[normalizedPath(skill.filePath)] || {}
  }

  applySkillOverrides(current, { includeDisabled = false } = {}) {
    return {
      diagnostics: current.diagnostics,
      skills: current.skills.flatMap((skill) => {
        const override = this.overrideFor(skill)
        if (!includeDisabled && override.enabled === false) return []
        const disableModelInvocation = typeof override.modelInvocation === 'boolean'
          ? !override.modelInvocation
          : Boolean(skill.disableModelInvocation)
        return [{ ...skill, disableModelInvocation }]
      }),
    }
  }

  invalidateDashboardCache() {
    this.dashboardCache = null
  }

  async createResourceLoader(cwd = this.cwd, { includeDisabled = false, appendSystemPrompt = '' } = {}) {
    const settingsManager = this.getSettingsManager()
    const loader = new DefaultResourceLoader({
      cwd,
      agentDir: this.agentDir,
      ...(settingsManager ? { settingsManager } : {}),
      ...(this.extensionFactories.length ? { extensionFactories: this.extensionFactories } : {}),
      skillsOverride: (current) => this.applySkillOverrides(current, { includeDisabled }),
      ...(appendSystemPrompt
        ? { appendSystemPromptOverride: (base) => [...base, appendSystemPrompt] }
        : {}),
    })
    await loader.reload()
    return loader
  }

  async resolveSkillPaths(cwd = this.cwd) {
    try {
      const resolved = await this.packageManager(cwd).resolve()
      return resolved.skills
        .filter((item) => item.enabled)
        .map((item) => mapSkillResourcePath(item))
        .filter(Boolean)
    } catch {
      // SettingsManager may not be ready during early bootstrap; fall back to default skill roots.
      return []
    }
  }

  async discover(cwd = this.cwd) {
    const skillPaths = await this.resolveSkillPaths(cwd)
    const loaded = loadSkills({
      cwd,
      agentDir: this.agentDir,
      skillPaths,
      // package resolve already auto-discovers user/project skill roots when settings are available.
      includeDefaults: skillPaths.length === 0,
    })
    return this.applySkillOverrides(loaded, { includeDisabled: true })
  }

  async publicSkill(skill) {
    const override = this.overrideFor(skill)
    const managed = this.state.installed[normalizedPath(skill.filePath)]
    let frontmatter = { version: 'latest', license: '', allowedTools: [] }
    try { frontmatter = parseFrontmatterDetails(await readFile(skill.filePath, 'utf8')) } catch {}
    const modelInvocationEnabled = typeof override.modelInvocation === 'boolean'
      ? override.modelInvocation
      : !skill.disableModelInvocation
    return {
      id: skillId(skill.filePath),
      name: skill.name,
      description: skill.description,
      filePath: skill.filePath,
      baseDir: skill.baseDir,
      enabled: override.enabled !== false,
      modelInvocationEnabled,
      command: this.getSettingsManager()?.getEnableSkillCommands?.() === false ? '' : `/skill:${skill.name}`,
      version: frontmatter.version,
      license: frontmatter.license,
      allowedTools: frontmatter.allowedTools,
      source: managed?.source || safeSourceLabel(sourceLabel(skill)),
      sourceInfo: skill.sourceInfo ? { ...skill.sourceInfo, source: safeSourceLabel(skill.sourceInfo.source) } : null,
      removable: Boolean(managed && pathInside(this.skillsDir, skill.filePath)),
    }
  }

  packageManager(cwd = this.cwd) {
    const settingsManager = this.getSettingsManager()
    if (!settingsManager) throw new Error('Pisper 技能运行时尚未初始化。')
    return this.createPackageManager({ cwd, agentDir: this.agentDir, settingsManager })
  }

  async buildDashboard(cwd = this.cwd) {
    const discovered = await this.discover(cwd)
    const skills = await Promise.all(discovered.skills.map((skill) => this.publicSkill(skill)))
    let packages = []
    try {
      packages = this.packageManager(cwd).listConfiguredPackages().map((item) => ({
        source: safeSourceLabel(item.source),
        scope: item.scope,
        filtered: item.filtered,
        installed: Boolean(item.installedPath),
      }))
    } catch {}
    return {
      skills,
      diagnostics: discovered.diagnostics.map((item) => ({ type: item.type, message: item.message, path: item.path || '' })),
      packages,
      counts: {
        installed: skills.length,
        enabled: skills.filter((skill) => skill.enabled).length,
        modelInvocable: skills.filter((skill) => skill.enabled && skill.modelInvocationEnabled).length,
      },
    }
  }

  async dashboard({ cwd = this.cwd, force = false } = {}) {
    const key = normalizedPath(cwd)
    const now = Date.now()
    if (!force && this.dashboardCache?.key === key && this.dashboardCache.expiresAt > now) {
      return clone(this.dashboardCache.value)
    }
    if (!force && this.dashboardInflight.has(key)) {
      return clone(await this.dashboardInflight.get(key))
    }

    const pending = this.buildDashboard(cwd)
      .then((value) => {
        this.dashboardCache = { key, value, expiresAt: Date.now() + DASHBOARD_CACHE_TTL_MS }
        return value
      })
      .finally(() => {
        if (this.dashboardInflight.get(key) === pending) this.dashboardInflight.delete(key)
      })

    this.dashboardInflight.set(key, pending)
    return clone(await pending)
  }

  async findSkill(id, cwd = this.cwd) {
    const discovered = await this.discover(cwd)
    return discovered.skills.find((skill) => skillId(skill.filePath) === id) || null
  }

  async update(id, input = {}, { cwd = this.cwd } = {}) {
    const skill = await this.findSkill(id, cwd)
    if (!skill) return null
    const key = normalizedPath(skill.filePath)
    const current = { ...(this.state.overrides[key] || {}) }
    if (typeof input.enabled === 'boolean') current.enabled = input.enabled
    if (typeof input.modelInvocationEnabled === 'boolean') current.modelInvocation = input.modelInvocationEnabled
    if (Object.keys(current).length) this.state.overrides[key] = current
    else delete this.state.overrides[key]
    await this.save()
    this.invalidateDashboardCache()
    return this.publicSkill(skill)
  }

  async resolveInstallSkills(source, cwd) {
    const localPath = expandPath(source, cwd)
    const localStat = localPath ? await pathExists(localPath) : null
    if (localStat) {
      await validateSkillSource(localPath)
      const loaded = loadSkills({ cwd, agentDir: this.agentDir, skillPaths: [localPath], includeDefaults: false })
      if (loaded.skills.length) return loaded
      throw new Error(loaded.diagnostics[0]?.message || '该本地路径没有发现符合 Agent Skills 标准的技能。')
    }

    const manager = this.packageManager(cwd)
    const resolved = await manager.resolveExtensionSources([source], { temporary: true })
    const paths = [...new Set(resolved.skills.filter((item) => item.enabled).map((item) => item.path))]
    if (!paths.length) throw new Error('该来源没有发现符合 Agent Skills 标准的技能。')
    return loadSkills({ cwd, agentDir: this.agentDir, skillPaths: paths, includeDefaults: false })
  }

  async copySkill(skill) {
    const skillName = slug(skill.name)
    if (basename(skill.filePath).toLowerCase() === 'skill.md') {
      const destination = join(this.skillsDir, skillName)
      if (await pathExists(destination)) throw new Error(`技能 ${skill.name} 已安装。`)
      await validateSkillSource(skill.baseDir)
      await cp(skill.baseDir, destination, { recursive: true, errorOnExist: true, force: false, dereference: false })
      return join(destination, 'SKILL.md')
    }
    const extension = extname(skill.filePath) || '.md'
    const destination = join(this.skillsDir, `${skillName}${extension}`)
    if (await pathExists(destination)) throw new Error(`技能 ${skill.name} 已安装。`)
    await validateSkillSource(skill.filePath)
    await cp(skill.filePath, destination, { errorOnExist: true, force: false, dereference: false })
    return destination
  }

  async install(input = {}, { cwd = this.cwd } = {}) {
    const source = String(input.source || '').trim()
    if (!source) throw new Error('请输入技能目录、SKILL.md、npm 包或 git 来源。')
    if (source.length > MAX_SKILL_SOURCE_CHARS) throw new Error('技能来源过长。')
    const loaded = await this.resolveInstallSkills(source, cwd)
    if (!loaded.skills.length) throw new Error('没有发现可安装技能。')
    if (loaded.skills.length > MAX_SKILLS_PER_INSTALL) throw new Error(`一次最多安装 ${MAX_SKILLS_PER_INSTALL} 个技能。`)
    const existingNames = new Set((await this.discover(cwd)).skills.map((skill) => skill.name))
    const duplicate = loaded.skills.find((skill) => existingNames.has(skill.name))
    if (duplicate) throw new Error(`技能 ${duplicate.name} 已存在，可直接启用或调用。`)
    const installedPaths = []
    try {
      for (const skill of loaded.skills) installedPaths.push(await this.copySkill(skill))
    } catch (error) {
      await Promise.allSettled(installedPaths.map((path) => rm(basename(path).toLowerCase() === 'skill.md' ? dirname(path) : path, { recursive: true, force: true })))
      throw error
    }
    const installedAt = new Date().toISOString()
    const installedSource = safeSourceLabel(source)
    for (const path of installedPaths) this.state.installed[normalizedPath(path)] = { source: installedSource, installedAt }
    try {
      await this.save()
    } catch (error) {
      for (const path of installedPaths) delete this.state.installed[normalizedPath(path)]
      await Promise.allSettled(installedPaths.map((path) => rm(basename(path).toLowerCase() === 'skill.md' ? dirname(path) : path, { recursive: true, force: true })))
      throw error
    }
    this.invalidateDashboardCache()
    const dashboard = await this.dashboard({ cwd, force: true })
    return {
      ...dashboard,
      installed: dashboard.skills.filter((skill) => installedPaths.some((path) => normalizedPath(path) === normalizedPath(skill.filePath))),
      source: installedSource,
    }
  }

  async remove(id, { cwd = this.cwd } = {}) {
    const skill = await this.findSkill(id, cwd)
    if (!skill) return false
    const key = normalizedPath(skill.filePath)
    if (!this.state.installed[key] || !pathInside(this.skillsDir, skill.filePath)) throw new Error('只能卸载由 Pisper 安装的技能；其他来源可以禁用。')
    const target = basename(skill.filePath).toLowerCase() === 'skill.md' ? dirname(skill.filePath) : skill.filePath
    await rm(target, { recursive: true, force: true })
    delete this.state.overrides[key]
    delete this.state.installed[key]
    await this.save()
    this.invalidateDashboardCache()
    return true
  }
}
