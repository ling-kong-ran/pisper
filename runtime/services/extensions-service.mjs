import { createHash } from 'node:crypto'
import { basename, extname, relative, resolve, sep } from 'node:path'
import { createDefaultPackageManager } from '../runtime/pi-coding-agent.mjs'

const MAX_SOURCE_CHARS = 2_000
const MAX_RUNTIME_DIAGNOSTICS = 100

function normalizedPath(value) {
  const path = resolve(String(value || ''))
  return process.platform === 'win32' ? path.toLowerCase() : path
}

function extensionId(resource) {
  return createHash('sha256')
    .update(
      [
        normalizedPath(resource.path),
        resource.metadata?.scope || '',
        resource.metadata?.source || '',
      ].join('\0'),
    )
    .digest('hex')
    .slice(0, 20)
}

function packageId(item) {
  return createHash('sha256').update(`${item.scope}\0${item.source}`).digest('hex').slice(0, 20)
}

function safeSource(value) {
  const source = String(value || '').trim()
  if (!/^(?:git\+)?[a-z][a-z\d+.-]*:\/\//i.test(source)) {
    return source.slice(0, MAX_SOURCE_CHARS)
  }
  const gitPrefix = /^git\+/i.test(source) ? source.slice(0, 4) : ''
  try {
    const url = new URL(gitPrefix ? source.slice(4) : source)
    if (url.username) url.username = '***'
    if (url.password) url.password = '***'
    for (const key of [...url.searchParams.keys()]) {
      if (/token|key|secret|password|auth/i.test(key)) url.searchParams.set(key, '***')
    }
    return `${gitPrefix}${url.toString()}`.slice(0, MAX_SOURCE_CHARS)
  } catch {
    return source.replace(/:\/\/[^/@\s]+@/, '://***@').slice(0, MAX_SOURCE_CHARS)
  }
}

function extensionName(path) {
  const file = basename(path)
  const name = file.slice(0, Math.max(0, file.length - extname(file).length)) || file
  return name === 'index' ? basename(resolve(path, '..')) : name
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function patternTarget(entry) {
  return /^[!+-]/.test(entry) ? entry.slice(1) : entry
}

function resourcePattern(resource) {
  const baseDir = resource.metadata?.baseDir
  if (!baseDir) return resource.path
  const path = relative(baseDir, resource.path)
  return path && !path.startsWith(`..${sep}`) && path !== '..'
    ? path.replace(/\\/g, '/')
    : resource.path
}

function capabilitySummary(extension, runtime) {
  const extensionPath = normalizedPath(extension?.resolvedPath || extension?.path)
  const providers = (runtime?.pendingProviderRegistrations || [])
    .filter((item) => normalizedPath(item.extensionPath) === extensionPath)
    .map((item) => item.name)
  return {
    tools: [...(extension?.tools?.keys?.() || [])],
    commands: [...(extension?.commands?.keys?.() || [])],
    events: [...(extension?.handlers?.keys?.() || [])],
    flags: [...(extension?.flags?.keys?.() || [])],
    shortcuts: [...(extension?.shortcuts?.keys?.() || [])],
    renderers: [
      ...(extension?.messageRenderers?.keys?.() || []),
      ...(extension?.entryRenderers?.keys?.() || []),
    ],
    providers,
  }
}

function diagnostic(input, defaults = {}) {
  return {
    type:
      input?.type === 'collision' ? 'collision' : input?.type === 'warning' ? 'warning' : 'error',
    phase: defaults.phase || 'load',
    message: String(input?.message || input?.error || 'Unknown Extension error'),
    path: String(input?.path || input?.extensionPath || ''),
    event: String(input?.event || ''),
    sessionId: String(defaults.sessionId || input?.sessionId || ''),
    timestamp: String(defaults.timestamp || input?.timestamp || ''),
    workspaceCwd: String(defaults.workspaceCwd || input?.workspaceCwd || ''),
  }
}

export class ExtensionsService {
  constructor({
    agentDir,
    cwd,
    getSettingsManager,
    createPackageManager,
    createResourceLoader,
  } = {}) {
    this.agentDir = agentDir
    this.cwd = cwd || process.cwd()
    this.getSettingsManager = getSettingsManager || (() => null)
    this.createPackageManager = createPackageManager || null
    this.createResourceLoader = createResourceLoader
    this.runtimeDiagnostics = []
    this.dashboardCache = new Map()
  }

  settingsManager(cwd = this.cwd) {
    const settingsManager = this.getSettingsManager(cwd)
    if (!settingsManager) throw new Error('Pisper Extension 运行时尚未初始化。')
    return settingsManager
  }

  async packageManager(cwd = this.cwd) {
    const options = { cwd, agentDir: this.agentDir, settingsManager: this.settingsManager(cwd) }
    return this.createPackageManager
      ? this.createPackageManager(options)
      : createDefaultPackageManager(options)
  }

  invalidate() {
    this.dashboardCache.clear()
  }

  recordRuntimeError(sessionId, cwd, error) {
    this.runtimeDiagnostics.push(
      diagnostic(error, {
        phase: 'event',
        sessionId,
        timestamp: new Date().toISOString(),
        workspaceCwd: cwd,
      }),
    )
    if (this.runtimeDiagnostics.length > MAX_RUNTIME_DIAGNOSTICS) {
      this.runtimeDiagnostics.splice(0, this.runtimeDiagnostics.length - MAX_RUNTIME_DIAGNOSTICS)
    }
    this.dashboardCache.delete(normalizedPath(cwd))
  }

  recordRuntime(sessionId, cwd, runner) {
    runner.getRegisteredCommands()
    const timestamp = new Date().toISOString()
    for (const item of runner.getCommandDiagnostics()) {
      this.runtimeDiagnostics.push(
        diagnostic(item, { phase: 'registration', sessionId, timestamp, workspaceCwd: cwd }),
      )
    }
    if (this.runtimeDiagnostics.length > MAX_RUNTIME_DIAGNOSTICS) {
      this.runtimeDiagnostics.splice(0, this.runtimeDiagnostics.length - MAX_RUNTIME_DIAGNOSTICS)
    }
    this.dashboardCache.delete(normalizedPath(cwd))
  }

  runtimeDiagnosticsFor(cwd) {
    const key = normalizedPath(cwd)
    return this.runtimeDiagnostics.filter(
      (item) => !item.workspaceCwd || normalizedPath(item.workspaceCwd) === key,
    )
  }

  async buildDashboard(cwd = this.cwd) {
    const manager = await this.packageManager(cwd)
    const [resolved, loader] = await Promise.all([
      manager.resolve(),
      this.createResourceLoader(cwd),
    ])
    const result = loader.getExtensions()
    const loadedByPath = new Map(
      result.extensions.map((extension) => [normalizedPath(extension.resolvedPath), extension]),
    )
    const diagnostics = result.errors.map((item) =>
      diagnostic(
        {
          ...item,
          type: /\bconflicts with\b/i.test(item.error) ? 'collision' : 'error',
        },
        { phase: 'load' },
      ),
    )
    diagnostics.push(
      ...this.runtimeDiagnosticsFor(cwd).map(({ workspaceCwd: _workspaceCwd, ...item }) => item),
    )
    const errorsByPath = new Map()
    for (const item of diagnostics) {
      if (!item.path) continue
      const key = normalizedPath(item.path)
      const current = errorsByPath.get(key) || []
      current.push(item)
      errorsByPath.set(key, current)
    }
    const extensions = resolved.extensions.map((resource) => {
      const loaded = loadedByPath.get(normalizedPath(resource.path))
      const itemDiagnostics = errorsByPath.get(normalizedPath(resource.path)) || []
      return {
        id: extensionId(resource),
        name: extensionName(resource.path),
        path: resource.path,
        enabled: resource.enabled,
        loaded: Boolean(loaded),
        scope: resource.metadata?.scope || 'user',
        origin: resource.metadata?.origin || 'top-level',
        source: safeSource(resource.metadata?.source || 'auto'),
        sourceInfo: {
          ...resource.metadata,
          source: safeSource(resource.metadata?.source || 'auto'),
        },
        capabilities: capabilitySummary(loaded, result.runtime),
        diagnosticCount: itemDiagnostics.length,
      }
    })
    const packages = manager.listConfiguredPackages().map((item) => ({
      id: packageId(item),
      source: safeSource(item.source),
      scope: item.scope,
      filtered: item.filtered,
      installed: Boolean(item.installedPath),
      extensionCount: resolved.extensions.filter(
        (resource) =>
          resource.metadata?.origin === 'package' &&
          resource.metadata?.scope === item.scope &&
          resource.metadata?.source === item.source,
      ).length,
    }))
    const settingsManager = this.settingsManager(cwd)
    return {
      cwd: resolve(cwd),
      trusted: settingsManager.isProjectTrusted?.() !== false,
      locations: {
        global: resolve(this.agentDir, 'extensions'),
        project: resolve(cwd, '.pi', 'extensions'),
      },
      extensions,
      packages,
      diagnostics,
      counts: {
        total: extensions.length,
        enabled: extensions.filter((item) => item.enabled).length,
        loaded: extensions.filter((item) => item.loaded).length,
        project: extensions.filter((item) => item.scope === 'project').length,
        errors: diagnostics.filter((item) => item.type === 'error').length,
        collisions: diagnostics.filter((item) => item.type === 'collision').length,
      },
    }
  }

  async dashboard({ cwd = this.cwd, force = false } = {}) {
    const key = normalizedPath(cwd)
    if (!force && this.dashboardCache.has(key)) return clone(this.dashboardCache.get(key))
    const value = await this.buildDashboard(cwd)
    this.dashboardCache.set(key, value)
    return clone(value)
  }

  assertProjectScope(scope, cwd) {
    if (scope !== 'project') return
    if (this.settingsManager(cwd).isProjectTrusted?.() === false) {
      throw new Error('请先信任当前工作区，再更改项目 Extension。')
    }
  }

  async install(input = {}, { cwd = this.cwd } = {}) {
    const source = String(input.source || '').trim()
    const scope = String(input.scope || 'user')
    if (!source) throw new Error('请输入本地路径、npm 包或 git 来源。')
    if (source.length > MAX_SOURCE_CHARS) throw new Error('Extension 来源过长。')
    if (!['user', 'project'].includes(scope)) throw new Error('Extension 作用域无效。')
    if (/^(?:git\+)?https?:\/\/[^/@\s]+@/i.test(source)) {
      throw new Error('Extension 来源不能包含内嵌凭据，请使用系统 Git 或 npm 凭据。')
    }
    this.assertProjectScope(scope, cwd)
    const manager = await this.packageManager(cwd)
    await manager.installAndPersist(source, { local: scope === 'project' })
    await this.settingsManager(cwd).flush()
    this.invalidate()
    return this.dashboard({ cwd, force: true })
  }

  async updatePackage(id, { cwd = this.cwd } = {}) {
    const manager = await this.packageManager(cwd)
    const configured = manager.listConfiguredPackages().find((item) => packageId(item) === id)
    if (!configured) return null
    this.assertProjectScope(configured.scope, cwd)
    await manager.update(configured.source)
    this.invalidate()
    return this.dashboard({ cwd, force: true })
  }

  async removePackage(id, { cwd = this.cwd } = {}) {
    const manager = await this.packageManager(cwd)
    const configured = manager.listConfiguredPackages().find((item) => packageId(item) === id)
    if (!configured) return false
    this.assertProjectScope(configured.scope, cwd)
    const removed = await manager.removeAndPersist(configured.source, {
      local: configured.scope === 'project',
    })
    await this.settingsManager(cwd).flush()
    this.invalidate()
    return removed
  }

  async updateExtension(id, enabled, { cwd = this.cwd } = {}) {
    const manager = await this.packageManager(cwd)
    const resolved = await manager.resolve()
    const resource = resolved.extensions.find((item) => extensionId(item) === id)
    if (!resource) return null
    const scope = resource.metadata?.scope === 'project' ? 'project' : 'user'
    this.assertProjectScope(scope, cwd)
    const settingsManager = this.settingsManager(cwd)
    const settings =
      scope === 'project'
        ? settingsManager.getProjectSettings()
        : settingsManager.getGlobalSettings()
    const pattern = resourcePattern(resource)
    const nextPattern = `${enabled ? '+' : '-'}${pattern}`
    if (resource.metadata?.origin === 'package') {
      const packages = [...(settings.packages || [])]
      const index = packages.findIndex(
        (item) => (typeof item === 'string' ? item : item.source) === resource.metadata.source,
      )
      if (index < 0) return null
      const entry =
        typeof packages[index] === 'string' ? { source: packages[index] } : { ...packages[index] }
      entry.extensions = [...(entry.extensions || [])].filter(
        (item) => patternTarget(item) !== pattern,
      )
      entry.extensions.push(nextPattern)
      packages[index] = entry
      if (scope === 'project') settingsManager.setProjectPackages(packages)
      else settingsManager.setPackages(packages)
    } else {
      const paths = [...(settings.extensions || [])].filter(
        (item) => patternTarget(item) !== pattern,
      )
      paths.push(nextPattern)
      if (scope === 'project') settingsManager.setProjectExtensionPaths(paths)
      else settingsManager.setExtensionPaths(paths)
    }
    await settingsManager.flush()
    this.invalidate()
    return this.dashboard({ cwd, force: true })
  }
}
