// 工具插件服务：管理工具插件的安装、启用与工具定义生成，
// 维护内置工具目录（TOOL_CATALOG）与第三方插件的合并视图。
import { createHash, randomUUID } from 'node:crypto'
import {
  cp,
  lstat,
  mkdir,
  open,
  readdir,
  readFile,
  realpath,
  rename,
  rm,
  rmdir,
  stat,
  writeFile,
} from 'node:fs/promises'
import { dirname, extname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { pathToFileURL } from 'node:url'
import { Type } from 'typebox'
import { Compile } from 'typebox/compile'

import { defineTool } from '../runtime/pi-coding-agent.mjs'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'
import {
  TOOL_CATALOG,
  TOOL_PRESETS,
  presetFromTools,
  sanitizeEnabledTools,
  toolsFromConfig,
} from '../tools/registry.mjs'
import { normalizeWebSearchConfig } from './web-search-service.mjs'
import { pisperError } from '../../shared/errors.mjs'

const STATE_VERSION = 1
const MANIFEST_FILE = 'pisper-plugin.json'
const MAX_PLUGIN_FILES = 512
const MAX_PLUGIN_BYTES = 20 * 1024 * 1024
const MAX_MANIFEST_BYTES = 256 * 1024
const MAX_RESULT_BYTES = 1024 * 1024
const INSPECTION_TTL_MS = 10 * 60 * 1000
const EXECUTION_TIMEOUT_MS = 2 * 60 * 1000
const PLUGIN_ID_PATTERN = /^[a-z0-9](?:[a-z0-9.-]{0,94}[a-z0-9])?$/
const TOOL_NAME_PATTERN = /^[a-z][a-z0-9_]{0,63}$/
const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/
const ALLOWED_ENTRY_EXTENSIONS = new Set(['.js', '.mjs', '.cjs'])
const WORKER_URL = new URL('../plugins/local-plugin-worker.mjs', import.meta.url)
const TOOL_IDS = new Set(TOOL_CATALOG.map((tool) => tool.id))
// 内置工具分组没有自己的说明，界面只显示分组名。
const BUILTIN_GROUP_DESCRIPTION = ''

function defaultState() {
  return { version: STATE_VERSION, plugins: {} }
}

function uniqueStrings(values) {
  return [
    ...new Set(
      (Array.isArray(values) ? values : [])
        .filter((value) => typeof value === 'string' && value.trim())
        .map((value) => value.trim()),
    ),
  ]
}

function normalizeRelativePath(value, field) {
  const normalized = String(value || '').replaceAll('\\', '/')
  if (
    !normalized ||
    isAbsolute(normalized) ||
    normalized === '..' ||
    normalized.startsWith('../') ||
    normalized.includes('/../')
  ) {
    throw pisperError('invalid', 'path', `${field} must be relative inside the plugin folder`)
  }
  return normalized
}

function isPathInside(root, candidate) {
  const rel = relative(root, candidate)
  return rel === '' || (!rel.startsWith(`..${sep}`) && rel !== '..' && !isAbsolute(rel))
}

function normalizeTool(tool, index) {
  if (!tool || typeof tool !== 'object' || Array.isArray(tool)) {
    throw pisperError('invalid', 'plugin', `tools[${index}] must be an object`)
  }
  const name = String(tool.name || '').trim()
  if (!TOOL_NAME_PATTERN.test(name)) {
    throw pisperError(
      'invalid',
      'tool',
      `${name || `tools[${index}]`}: name is a lowercase letter then lowercase letters, digits or underscores`,
    )
  }
  const label = String(tool.label || name).trim()
  const description = String(tool.description || '').trim()
  if (!description) throw pisperError('invalid', 'tool', `${name}: description is required`)
  const parameters =
    tool.parameters && typeof tool.parameters === 'object' && !Array.isArray(tool.parameters)
      ? tool.parameters
      : { type: 'object', properties: {} }
  if (parameters.type !== 'object') {
    throw pisperError('invalid', 'tool', `${name}: parameters.type must be object`)
  }
  if (
    parameters.properties != null &&
    (typeof parameters.properties !== 'object' || Array.isArray(parameters.properties))
  ) {
    throw pisperError('invalid', 'tool', `${name}: parameters.properties must be an object`)
  }
  if (
    parameters.required != null &&
    (!Array.isArray(parameters.required) ||
      parameters.required.some((property) => typeof property !== 'string'))
  ) {
    throw pisperError('invalid', 'tool', `${name}: parameters.required must be a string array`)
  }
  try {
    Compile(Type.Unsafe(parameters))
  } catch {
    throw pisperError('invalid', 'tool', `${name}: parameters is not a valid JSON Schema`)
  }
  return {
    name,
    label: label.slice(0, 100),
    description: description.slice(0, 1000),
    scope: String(tool.scope || description)
      .trim()
      .slice(0, 500),
    parameters,
  }
}

function normalizeManifest(raw) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw))
    throw pisperError('invalid', 'plugin', 'manifest must be a JSON object')
  const schemaVersion = Number(raw.schemaVersion || 1)
  if (schemaVersion !== 1)
    throw pisperError('unsupported', 'plugin', `manifest schemaVersion ${schemaVersion}`)
  const id = String(raw.id || '').trim()
  if (!PLUGIN_ID_PATTERN.test(id))
    throw pisperError('invalid', 'plugin', 'id is 1-96 lowercase letters, digits, dots or hyphens')
  const name = String(raw.name || '').trim()
  if (!name || name.length > 100) throw pisperError('invalid', 'plugin', 'name is 1-100 characters')
  const version = String(raw.version || '').trim()
  if (!VERSION_PATTERN.test(version))
    throw pisperError('invalid', 'plugin', 'version must be semver, e.g. 1.0.0')
  const entry = normalizeRelativePath(raw.entry, 'entry')
  if (!ALLOWED_ENTRY_EXTENSIONS.has(extname(entry).toLowerCase()))
    throw pisperError('invalid', 'plugin', 'entry must be a .js, .mjs or .cjs file')
  const tools = (Array.isArray(raw.tools) ? raw.tools : []).map(normalizeTool)
  if (tools.length === 0 || tools.length > 32)
    throw pisperError('invalid', 'plugin', 'declare 1-32 tools')
  if (new Set(tools.map((tool) => tool.name)).size !== tools.length)
    throw pisperError('invalid', 'plugin', 'tool names must be unique')
  return {
    schemaVersion,
    id,
    name,
    version,
    description: String(raw.description || '')
      .trim()
      .slice(0, 1000),
    entry,
    permissions: uniqueStrings(raw.permissions).slice(0, 32),
    tools,
  }
}

async function readManifest(root) {
  let raw
  try {
    const manifestPath = join(root, MANIFEST_FILE)
    const info = await lstat(manifestPath)
    if (info.isSymbolicLink() || !info.isFile())
      throw pisperError('invalid', 'plugin', `${MANIFEST_FILE} must be a regular file`)
    if (info.size > MAX_MANIFEST_BYTES)
      throw pisperError('too_large', 'plugin', `${MANIFEST_FILE} max 256 KB`)
    raw = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') throw pisperError('not_found', 'file', MANIFEST_FILE)
    if (error instanceof SyntaxError)
      throw pisperError('invalid', 'plugin', `${MANIFEST_FILE} is not valid JSON`)
    throw error
  }
  return normalizeManifest(raw)
}

async function scanDirectory(root) {
  const hash = createHash('sha256')
  let fileCount = 0
  let byteCount = 0

  async function visit(directory) {
    const entries = await readdir(directory, { withFileTypes: true })
    entries.sort((left, right) => left.name.localeCompare(right.name))
    for (const entry of entries) {
      const absolutePath = join(directory, entry.name)
      const relativePath = relative(root, absolutePath).split(sep).join('/')
      if (entry.isSymbolicLink())
        throw pisperError('forbidden', 'plugin', `symbolic link: ${relativePath}`)
      if (entry.isDirectory()) {
        await visit(absolutePath)
        continue
      }
      if (!entry.isFile())
        throw pisperError('unsupported', 'plugin', `special file: ${relativePath}`)
      const info = await stat(absolutePath)
      fileCount += 1
      byteCount += info.size
      if (fileCount > MAX_PLUGIN_FILES)
        throw pisperError('too_large', 'plugin', `max ${MAX_PLUGIN_FILES} files`)
      if (byteCount > MAX_PLUGIN_BYTES) throw pisperError('too_large', 'plugin', 'max 20 MB')
      hash.update(relativePath)
      hash.update('\0')
      hash.update(await readFile(absolutePath))
      hash.update('\0')
    }
  }

  await visit(root)
  return { digest: hash.digest('hex'), fileCount, byteCount }
}

function builtInPlugins(tools, enabledSet) {
  const groups = new Map()
  for (const tool of tools) {
    const category = tool.category || 'system'
    if (!groups.has(category)) groups.set(category, [])
    groups.get(category).push({
      ...tool,
      name: tool.id,
      label: tool.name,
      enabled: enabledSet.has(tool.id),
      effectiveRisk: tool.risk,
    })
  }
  return [...groups.entries()].map(([category, capabilities]) => ({
    id: `builtin.${category}`,
    name: category,
    description: BUILTIN_GROUP_DESCRIPTION,
    version: 'builtin',
    source: 'builtin',
    builtIn: true,
    enabled: capabilities.some((capability) => capability.enabled),
    capabilities,
  }))
}

function publicInstalledPlugin(record, manifest) {
  const enabledSet = new Set(record.enabledTools || [])
  const capabilities = manifest.tools.map((tool) => ({
    ...tool,
    category: 'integration',
    risk: 'high',
    effectiveRisk: 'high',
    enabled: enabledSet.has(tool.name),
  }))
  return {
    id: manifest.id,
    name: manifest.name,
    description: manifest.description,
    version: manifest.version,
    source: 'local',
    builtIn: false,
    enabled: capabilities.some((capability) => capability.enabled),
    permissions: manifest.permissions,
    systemAccess: true,
    installedAt: record.installedAt,
    digest: record.digest,
    capabilities,
  }
}

function normalizeToolResult(result) {
  let normalized
  if (typeof result === 'string')
    normalized = { content: [{ type: 'text', text: result }], details: {} }
  else if (result && typeof result === 'object' && Array.isArray(result.content))
    normalized = result
  else
    normalized = {
      content: [{ type: 'text', text: JSON.stringify(result ?? null, null, 2) }],
      details: {},
    }
  let serialized
  try {
    serialized = JSON.stringify(normalized)
  } catch {
    throw pisperError('invalid', 'plugin', 'result is not serializable')
  }
  if (Buffer.byteLength(serialized, 'utf8') > MAX_RESULT_BYTES)
    throw pisperError('too_large', 'plugin', 'result max 1 MB')
  return normalized
}

export class ToolPluginService {
  constructor(configPath, options = {}) {
    this.configPath = configPath
    this.dataDir = options.dataDir || dirname(configPath)
    this.pluginRoot = options.pluginRoot || join(this.dataDir, 'plugins')
    this.statePath = options.statePath || join(this.dataDir, 'pisper-plugins.json')
    this.state = defaultState()
    this.installed = new Map()
    this.inspections = new Map()
    this.activeExecutions = new Map()
    this.changingPlugins = new Set()
    this.workers = new Set()
    this.disposed = false
    this.createWrite = Promise.resolve()
  }

  async init() {
    // 加载插件状态（含内置工具默认开关）。
    await mkdir(this.pluginRoot, { recursive: true })
    const pluginRootInfo = await lstat(this.pluginRoot)
    if (pluginRootInfo.isSymbolicLink() || !pluginRootInfo.isDirectory()) {
      throw pisperError('invalid', 'folder', 'plugin root must be a regular folder')
    }
    try {
      const parsed = JSON.parse(await readFile(this.statePath, 'utf8'))
      if (parsed?.version === STATE_VERSION && parsed.plugins && typeof parsed.plugins === 'object')
        this.state = parsed
    } catch (error) {
      if (error?.code !== 'ENOENT') console.warn('[plugins] failed to load plugin state:', error)
    }
    await this.#refreshInstalled()
  }

  dispose() {
    this.disposed = true
    this.inspections.clear()
    this.changingPlugins.clear()
    for (const worker of this.workers) void worker.terminate()
    this.workers.clear()
  }

  // 注册默认工具开关（幂等）：首次运行时启用指定内置工具。
  async ensureDefaultTools(toolIds, migrationKey, { skip } = {}) {
    const appConfig = await readJson(this.configPath, { toolMode: 'full' })
    if (appConfig[migrationKey]) return
    if (skip?.(appConfig)) {
      await writeJsonAtomic(this.configPath, { ...appConfig, [migrationKey]: true })
      return
    }
    const enabledTools = [
      ...new Set([...toolsFromConfig(appConfig), ...sanitizeEnabledTools(toolIds)]),
    ]
    await writeJsonAtomic(this.configPath, {
      ...appConfig,
      toolMode: presetFromTools(enabledTools),
      enabledTools,
      [migrationKey]: true,
    })
  }

  async #refreshInstalled() {
    this.installed.clear()
    for (const [id, record] of Object.entries(this.state.plugins)) {
      try {
        if (!PLUGIN_ID_PATTERN.test(id) || !record || typeof record !== 'object') {
          throw pisperError('invalid', 'plugin', 'invalid install record')
        }
        const root = join(this.pluginRoot, id, String(record.version || ''))
        const manifest = await readManifest(root)
        if (manifest.id !== id || manifest.version !== record.version)
          throw pisperError('invalid', 'plugin', 'manifest does not match install record')
        this.installed.set(id, { root, manifest, record })
      } catch (error) {
        console.warn(`[plugins] failed to load ${id}:`, error)
      }
    }
  }

  async #writeState() {
    await writeJsonAtomic(this.statePath, this.state)
  }

  #validateToolConflicts(manifest, ignoredPluginId = '') {
    for (const tool of manifest.tools) {
      if (TOOL_IDS.has(tool.name))
        throw pisperError('exists', 'tool', `${tool.name} is a built-in tool`)
      for (const [pluginId, installed] of this.installed) {
        if (
          pluginId !== ignoredPluginId &&
          installed.manifest.tools.some((candidate) => candidate.name === tool.name)
        ) {
          throw pisperError('exists', 'tool', `${tool.name} is provided by plugin ${pluginId}`)
        }
      }
    }
  }

  // 检查插件包：解析清单/入口/能力，生成可安装的检查结果。
  async inspect(sourcePath) {
    const requestedPath = String(sourcePath || '').trim()
    if (!requestedPath) throw pisperError('invalid', 'folder', 'choose a plugin folder')
    const requestedRoot = resolve(requestedPath)
    const requestedInfo = await lstat(requestedRoot)
    if (requestedInfo.isSymbolicLink() || !requestedInfo.isDirectory()) {
      throw pisperError('invalid', 'folder', 'plugin source must be a folder, not a link')
    }
    const sourceRoot = await realpath(requestedRoot)
    const manifest = await readManifest(sourceRoot)
    const entryPath = await realpath(join(sourceRoot, manifest.entry))
    if (!isPathInside(sourceRoot, entryPath) || !(await stat(entryPath)).isFile())
      throw pisperError('invalid', 'plugin', 'entry must be a regular file inside the folder')
    if (this.installed.has(manifest.id) || this.state.plugins[manifest.id])
      throw pisperError('exists', 'plugin', manifest.id)
    this.#validateToolConflicts(manifest)
    const scan = await scanDirectory(sourceRoot)
    const inspectionId = randomUUID()
    this.inspections.set(inspectionId, {
      sourceRoot,
      manifest,
      scan,
      expiresAt: Date.now() + INSPECTION_TTL_MS,
    })
    return {
      inspectionId,
      plugin: publicInstalledPlugin(
        { enabledTools: manifest.tools.map((tool) => tool.name) },
        manifest,
      ),
      fileCount: scan.fileCount,
      byteCount: scan.byteCount,
      digest: scan.digest,
      warnings: [
        'Plugin code runs in a separate worker but can still reach local files and the network as the current user.',
        'Third-party plugin tools are available in every execution mode; outside Full access each call needs confirmation.',
      ],
    }
  }

  // 安装已检查的插件：解包到插件目录并激活。
  async install(inspectionId) {
    const inspection = this.inspections.get(String(inspectionId || ''))
    if (!inspection || inspection.expiresAt < Date.now())
      throw pisperError('invalid', 'plugin', 'inspection expired; inspect the folder again')
    const { sourceRoot, manifest, scan } = inspection
    if (this.installed.has(manifest.id) || this.state.plugins[manifest.id])
      throw pisperError('exists', 'plugin', manifest.id)
    this.#validateToolConflicts(manifest)
    if (this.changingPlugins.has(manifest.id)) throw pisperError('busy', 'plugin')
    this.changingPlugins.add(manifest.id)
    try {
      const currentScan = await scanDirectory(sourceRoot)
      if (currentScan.digest !== scan.digest)
        throw pisperError('invalid', 'plugin', 'folder changed after inspection; inspect it again')

      const pluginDirectory = join(this.pluginRoot, manifest.id)
      const destination = join(pluginDirectory, manifest.version)
      const stage = `${destination}.install-${randomUUID()}`
      await mkdir(pluginDirectory, { recursive: true })
      const pluginDirectoryInfo = await lstat(pluginDirectory)
      if (pluginDirectoryInfo.isSymbolicLink() || !pluginDirectoryInfo.isDirectory()) {
        throw pisperError('invalid', 'folder', 'plugin folder must be a regular folder')
      }
      try {
        await open(destination, 'wx')
          .then((handle) => handle.close())
          .catch((error) => {
            if (error?.code === 'EISDIR' || error?.code === 'EEXIST')
              throw pisperError('exists', 'plugin', `${manifest.id}@${manifest.version}`)
            throw error
          })
        await rm(destination, { force: true })
        await cp(sourceRoot, stage, { recursive: true, errorOnExist: true, force: false })
        const copiedScan = await scanDirectory(stage)
        if (copiedScan.digest !== scan.digest)
          throw pisperError('invalid', 'plugin', 'folder changed while copying; inspect it again')
        await rename(stage, destination)
        const record = {
          version: manifest.version,
          installedAt: new Date().toISOString(),
          digest: scan.digest,
          enabledTools: manifest.tools.map((tool) => tool.name),
        }
        this.state.plugins[manifest.id] = record
        try {
          await this.#writeState()
        } catch (error) {
          delete this.state.plugins[manifest.id]
          await rm(destination, { recursive: true, force: true })
          throw error
        }
        this.installed.set(manifest.id, { root: destination, manifest, record })
        this.inspections.delete(String(inspectionId))
        return publicInstalledPlugin(record, manifest)
      } catch (error) {
        await rm(stage, { recursive: true, force: true })
        throw error
      }
    } finally {
      this.changingPlugins.delete(manifest.id)
    }
  }

  async create(input = {}) {
    const task = () => this.#createValidatedPlugin(input)
    this.createWrite = this.createWrite.catch(() => {}).then(task)
    return this.createWrite
  }

  async #createValidatedPlugin(input) {
    const entryCode = String(input.entryCode || '')
    if (!entryCode.trim()) throw pisperError('invalid', 'plugin', 'entryCode is required')
    const manifest = normalizeManifest({
      schemaVersion: 1,
      id: input.id,
      name: input.name,
      version: input.version || '1.0.0',
      description: input.description,
      entry: 'index.mjs',
      permissions: input.permissions,
      tools: input.tools,
    })
    if (this.installed.has(manifest.id) || this.state.plugins[manifest.id]) {
      throw pisperError('exists', 'plugin', manifest.id)
    }

    const extraFiles = Array.isArray(input.files) ? input.files : []
    if (extraFiles.length > 64) throw pisperError('limit', 'file', 'max 64 extra files')
    const normalizedFiles = []
    const seenPaths = new Set([MANIFEST_FILE, manifest.entry])
    for (const [index, file] of extraFiles.entries()) {
      if (!file || typeof file !== 'object' || Array.isArray(file)) {
        throw pisperError('invalid', 'file', `files[${index}] must be an object`)
      }
      const path = normalizeRelativePath(file.path, `files[${index}].path`).replace(/^\.\//, '')
      if (path === '.' || path.endsWith('/') || seenPaths.has(path)) {
        throw pisperError('invalid', 'path', `duplicate or invalid: ${path}`)
      }
      const content = String(file.content ?? '')
      seenPaths.add(path)
      normalizedFiles.push({ path, content })
    }

    const manifestContent = `${JSON.stringify(manifest, null, 2)}\n`
    const generatedFiles = [
      { path: MANIFEST_FILE, content: manifestContent },
      { path: manifest.entry, content: entryCode },
      ...normalizedFiles,
    ]
    const generatedBytes = generatedFiles.reduce(
      (total, file) => total + Buffer.byteLength(file.content, 'utf8'),
      0,
    )
    if (generatedBytes > MAX_PLUGIN_BYTES) throw pisperError('too_large', 'plugin', 'max 20 MB')

    const canonicalDataDir = await realpath(this.dataDir)
    const sourceRoot = join(canonicalDataDir, 'plugin-sources')
    await mkdir(sourceRoot).catch((error) => {
      if (error?.code !== 'EEXIST') throw error
    })
    const sourceRootInfo = await lstat(sourceRoot)
    if (sourceRootInfo.isSymbolicLink() || !sourceRootInfo.isDirectory()) {
      throw pisperError('invalid', 'folder', 'plugin sources folder must be a regular folder')
    }
    const canonicalSourceRoot = await realpath(sourceRoot)
    if (!isPathInside(canonicalDataDir, canonicalSourceRoot)) {
      throw pisperError(
        'forbidden',
        'folder',
        'plugin sources folder links outside the data folder',
      )
    }

    const destination = join(canonicalSourceRoot, manifest.id)
    try {
      await mkdir(destination)
    } catch (error) {
      if (error?.code === 'EEXIST')
        throw pisperError('exists', 'folder', `plugin-sources/${manifest.id}`)
      throw error
    }

    const createdFiles = []
    const createdDirectories = []
    const ensureParent = async (relativePath) => {
      const parts = dirname(relativePath)
        .split('/')
        .filter((part) => part && part !== '.')
      let current = destination
      for (const part of parts) {
        current = join(current, part)
        try {
          await mkdir(current)
          createdDirectories.push(current)
        } catch (error) {
          if (error?.code !== 'EEXIST') throw error
        }
      }
    }
    try {
      for (const file of generatedFiles) {
        await ensureParent(file.path)
        const filePath = join(destination, file.path)
        await writeFile(filePath, file.content, { encoding: 'utf8', flag: 'wx' })
        createdFiles.push({ path: filePath, content: Buffer.from(file.content, 'utf8') })
      }
      const inspection = await this.inspect(destination)
      const installed = await this.install(inspection.inspectionId)
      return {
        id: manifest.id,
        name: manifest.name,
        version: manifest.version,
        sourcePath: destination,
        tools: manifest.tools.map((tool) => tool.name),
        installed,
      }
    } catch (error) {
      for (const file of createdFiles.reverse()) {
        const current = await readFile(file.path).catch(() => null)
        if (current && Buffer.compare(current, file.content) === 0)
          await rm(file.path, { force: true })
      }
      for (const directory of createdDirectories.reverse()) await rmdir(directory).catch(() => {})
      await rmdir(destination).catch(() => {})
      throw error
    }
  }

  async getState() {
    let appConfig = {}
    try {
      appConfig = JSON.parse(await readFile(this.configPath, 'utf8'))
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    const enabledBuiltIns = toolsFromConfig(appConfig)
    const enabledSet = new Set(enabledBuiltIns)
    const plugins = [
      ...builtInPlugins(TOOL_CATALOG, enabledSet),
      ...[...this.installed.values()].map(({ record, manifest }) =>
        publicInstalledPlugin(record, manifest),
      ),
    ]
    const enabledTools = [
      ...enabledBuiltIns,
      ...plugins
        .filter((plugin) => !plugin.builtIn)
        .flatMap((plugin) =>
          plugin.capabilities
            .filter((capability) => capability.enabled)
            .map((capability) => capability.name),
        ),
    ]
    return {
      plugins,
      tools: plugins.flatMap((plugin) =>
        plugin.capabilities.map((capability) => ({
          ...capability,
          id: capability.id || capability.name,
          name: capability.label || capability.name,
          pluginId: plugin.id,
          pluginName: plugin.name,
          source: plugin.builtIn ? capability.source : plugin.source,
        })),
      ),
      presets: TOOL_PRESETS,
      enabledTools,
      preset: presetFromTools(enabledBuiltIns),
      changes: Array.isArray(appConfig.pluginChanges) ? appConfig.pluginChanges.slice(0, 20) : [],
      updatedAt: appConfig.pluginsUpdatedAt || null,
      webSearch: normalizeWebSearchConfig(appConfig.webSearch || {}),
      piExtensions:
        appConfig.piExtensions && typeof appConfig.piExtensions === 'object'
          ? { ...appConfig.piExtensions }
          : {},
    }
  }

  async saveState(input = {}) {
    const current = await this.getState()
    const requested = uniqueStrings(input.enabledTools)
    const allowed = new Set(current.tools.map((tool) => tool.id))
    const enabledTools = requested.filter((name) => allowed.has(name))
    const enabledSet = new Set(enabledTools)
    const builtInEnabled = enabledTools.filter((name) => TOOL_IDS.has(name))
    let config = {}
    try {
      config = JSON.parse(await readFile(this.configPath, 'utf8'))
    } catch (error) {
      if (error?.code !== 'ENOENT') throw error
    }
    const now = new Date().toISOString()
    const webSearch = Object.hasOwn(input, 'webSearch')
      ? normalizeWebSearchConfig(input.webSearch)
      : normalizeWebSearchConfig(config.webSearch || {})
    const piExtensions =
      input.piExtensions && typeof input.piExtensions === 'object'
        ? { ...input.piExtensions }
        : config.piExtensions && typeof config.piExtensions === 'object'
          ? { ...config.piExtensions }
          : {}
    const toolMetadata = new Map(current.tools.map((tool) => [tool.id, tool]))
    const changes = [
      ...enabledTools.filter((name) => !current.enabledTools.includes(name)),
      ...current.enabledTools.filter((name) => !enabledSet.has(name)),
    ].map((name) => ({
      tool: name,
      name: toolMetadata.get(name)?.label || name,
      enabled: enabledSet.has(name),
      timestamp: now,
    }))
    const previousEnabledTools = new Map(
      [...this.installed].map(([pluginId, installed]) => [
        pluginId,
        [...installed.record.enabledTools],
      ]),
    )
    for (const installed of this.installed.values()) {
      installed.record.enabledTools = installed.manifest.tools
        .filter((tool) => enabledSet.has(tool.name))
        .map((tool) => tool.name)
      this.state.plugins[installed.manifest.id] = installed.record
    }
    try {
      await this.#writeState()
      await writeJsonAtomic(this.configPath, {
        ...config,
        toolMode: presetFromTools(builtInEnabled),
        enabledTools: builtInEnabled,
        pluginChanges: [...changes, ...(config.pluginChanges || [])].slice(0, 50),
        pluginsUpdatedAt: now,
        webSearch,
        piExtensions,
      })
    } catch (error) {
      for (const [pluginId, installed] of this.installed) {
        installed.record.enabledTools = previousEnabledTools.get(pluginId) || []
        this.state.plugins[pluginId] = installed.record
      }
      await this.#writeState().catch(() => {})
      throw error
    }
    return {
      ...(await this.getState()),
    }
  }

  async setPluginEnabled(pluginId, enabled) {
    const current = await this.getState()
    const plugin = current.plugins.find((candidate) => candidate.id === pluginId)
    if (!plugin) throw pisperError('not_found', 'plugin', pluginId)
    const capabilityNames = new Set(plugin.capabilities.map((capability) => capability.name))
    const enabledTools = enabled
      ? [...new Set([...current.enabledTools, ...capabilityNames])]
      : current.enabledTools.filter((name) => !capabilityNames.has(name))
    return await this.saveState({ enabledTools })
  }

  async setCapabilityEnabled(pluginId, toolName, enabled) {
    const current = await this.getState()
    const plugin = current.plugins.find((candidate) => candidate.id === pluginId)
    if (!plugin?.capabilities.some((capability) => capability.name === toolName))
      throw pisperError('not_found', 'tool', `${pluginId} has no ${toolName}`)
    const enabledTools = enabled
      ? [...new Set([...current.enabledTools, toolName])]
      : current.enabledTools.filter((name) => name !== toolName)
    return await this.saveState({ enabledTools })
  }

  async uninstall(pluginId) {
    const installed = this.installed.get(pluginId)
    if (!installed) throw pisperError('not_found', 'plugin', pluginId)
    if (this.changingPlugins.has(pluginId)) throw pisperError('busy', 'plugin')
    this.changingPlugins.add(pluginId)
    const pluginDirectory = join(this.pluginRoot, pluginId)
    const quarantine = join(this.pluginRoot, `.uninstall-${pluginId}-${randomUUID()}`)
    let quarantined = false
    try {
      if ((this.activeExecutions.get(pluginId) || 0) > 0) throw pisperError('busy', 'plugin')
      await rename(pluginDirectory, quarantine)
      quarantined = true
      const previousRecord = this.state.plugins[pluginId]
      delete this.state.plugins[pluginId]
      try {
        await this.#writeState()
      } catch (error) {
        this.state.plugins[pluginId] = previousRecord
        try {
          await rename(quarantine, pluginDirectory)
          quarantined = false
        } catch {
          // The outer finally retries restoration before releasing the lifecycle guard.
        }
        throw error
      }
      this.installed.delete(pluginId)
      await rm(quarantine, { recursive: true, force: true }).catch((error) => {
        console.warn(`[plugins] failed to remove uninstall quarantine for ${pluginId}:`, error)
      })
      quarantined = false
      return await this.getState()
    } finally {
      if (quarantined) await rename(quarantine, pluginDirectory).catch(() => {})
      this.changingPlugins.delete(pluginId)
    }
  }

  getToolRisk(name) {
    return this.isThirdPartyTool(name) ? 'high' : undefined
  }

  isThirdPartyTool(name) {
    for (const installed of this.installed.values()) {
      if (installed.manifest.tools.some((tool) => tool.name === name)) return true
    }
    return false
  }

  // 启用的工具：内置工具按配置，第三方插件按各自记录。不随执行模式变化——
  // 第三方插件工具在完全访问以外的模式下逐次确认（session-permission-service）。
  enabledTools(appConfig) {
    const builtIns = toolsFromConfig(appConfig)
    return [
      ...builtIns,
      ...[...this.installed.values()].flatMap(({ record }) => record.enabledTools || []),
    ]
  }

  createToolDefinitions({ cwd, sessionId, enabledTools }) {
    const enabledSet = new Set(enabledTools)
    const definitions = []
    for (const [pluginId, installed] of this.installed) {
      for (const tool of installed.manifest.tools) {
        if (!enabledSet.has(tool.name) || !installed.record.enabledTools.includes(tool.name))
          continue
        definitions.push(
          defineTool({
            name: tool.name,
            label: tool.label,
            description: tool.description,
            parameters: Type.Unsafe(tool.parameters),
            category: 'integration',
            risk: 'high',
            scope: tool.scope,
            execute: async (_toolCallId, params, signal) =>
              await this.#execute(pluginId, installed, tool.name, params, {
                cwd,
                sessionId,
                dataDir: join(this.dataDir, 'plugin-data', pluginId),
                signal,
              }),
          }),
        )
      }
    }
    return definitions
  }

  async #execute(pluginId, installed, toolName, arguments_, context) {
    if (this.disposed) throw pisperError('unavailable', 'plugin', 'plugin service stopped')
    if (this.changingPlugins.has(pluginId)) throw pisperError('busy', 'plugin')
    this.activeExecutions.set(pluginId, (this.activeExecutions.get(pluginId) || 0) + 1)
    let worker
    try {
      await mkdir(context.dataDir, { recursive: true })
      const { Worker } = await import('node:worker_threads')
      const entryPath = join(installed.root, installed.manifest.entry)
      worker = new Worker(WORKER_URL, {
        workerData: {
          entryUrl: pathToFileURL(entryPath).href,
          toolName,
          arguments: arguments_ ?? {},
          context: { cwd: context.cwd, sessionId: context.sessionId, dataDir: context.dataDir },
        },
        env: {},
        stdout: true,
        stderr: true,
        resourceLimits: { maxOldGenerationSizeMb: 128 },
      })
      this.workers.add(worker)
      worker.stdout?.resume()
      worker.stderr?.resume()
      return await new Promise((resolveResult, rejectResult) => {
        let settled = false
        const finish = (callback, value) => {
          if (settled) return
          settled = true
          clearTimeout(timer)
          context.signal?.removeEventListener('abort', abort)
          callback(value)
        }
        const abort = () => {
          void worker.terminate()
          finish(rejectResult, pisperError('cancelled', 'plugin'))
        }
        const timer = setTimeout(() => {
          void worker.terminate()
          finish(rejectResult, pisperError('timeout', 'plugin', 'max 120 seconds'))
        }, EXECUTION_TIMEOUT_MS)
        worker.on('message', (message) => {
          if (message?.type === 'result') finish(resolveResult, normalizeToolResult(message.result))
          else if (message?.type === 'error')
            finish(
              rejectResult,
              message.error?.message
                ? new Error(message.error.message)
                : pisperError('failed', 'plugin'),
            )
        })
        worker.on('error', (error) => finish(rejectResult, error))
        worker.on('exit', (code) => {
          if (code !== 0)
            finish(rejectResult, pisperError('failed', 'plugin', `worker exited with code ${code}`))
        })
        if (context.signal?.aborted) abort()
        else context.signal?.addEventListener('abort', abort, { once: true })
      })
    } finally {
      if (worker) this.workers.delete(worker)
      await worker?.terminate().catch(() => {})
      const remaining = (this.activeExecutions.get(pluginId) || 1) - 1
      if (remaining > 0) this.activeExecutions.set(pluginId, remaining)
      else this.activeExecutions.delete(pluginId)
    }
  }
}
