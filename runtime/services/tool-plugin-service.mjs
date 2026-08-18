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
import { Worker } from 'node:worker_threads'
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
    throw new Error(`${field} 必须是插件目录内的相对路径。`)
  }
  return normalized
}

function isPathInside(root, candidate) {
  const rel = relative(root, candidate)
  return rel === '' || (!rel.startsWith(`..${sep}`) && rel !== '..' && !isAbsolute(rel))
}

function normalizeTool(tool, index) {
  if (!tool || typeof tool !== 'object' || Array.isArray(tool)) {
    throw new Error(`tools[${index}] 必须是对象。`)
  }
  const name = String(tool.name || '').trim()
  if (!TOOL_NAME_PATTERN.test(name)) {
    throw new Error(
      `工具名称 ${name || `(index ${index})`} 无效；必须以小写字母开头，且只能包含小写字母、数字和下划线。`,
    )
  }
  const label = String(tool.label || name).trim()
  const description = String(tool.description || '').trim()
  if (!description) throw new Error(`工具 ${name} 缺少 description。`)
  const parameters =
    tool.parameters && typeof tool.parameters === 'object' && !Array.isArray(tool.parameters)
      ? tool.parameters
      : { type: 'object', properties: {} }
  if (parameters.type !== 'object') {
    throw new Error(`工具 ${name} 的 parameters.type 必须为 object。`)
  }
  if (
    parameters.properties != null &&
    (typeof parameters.properties !== 'object' || Array.isArray(parameters.properties))
  ) {
    throw new Error(`工具 ${name} 的 parameters.properties 必须为对象。`)
  }
  if (
    parameters.required != null &&
    (!Array.isArray(parameters.required) ||
      parameters.required.some((property) => typeof property !== 'string'))
  ) {
    throw new Error(`工具 ${name} 的 parameters.required 必须为字符串数组。`)
  }
  try {
    Compile(Type.Unsafe(parameters))
  } catch {
    throw new Error(`工具 ${name} 的 parameters 不是有效的 JSON Schema。`)
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
    throw new Error('插件清单必须是 JSON 对象。')
  const schemaVersion = Number(raw.schemaVersion || 1)
  if (schemaVersion !== 1) throw new Error(`不支持插件清单版本 ${schemaVersion}。`)
  const id = String(raw.id || '').trim()
  if (!PLUGIN_ID_PATTERN.test(id))
    throw new Error('插件 id 必须为 1-96 位小写字母、数字、点或连字符。')
  const name = String(raw.name || '').trim()
  if (!name || name.length > 100) throw new Error('插件 name 必须为 1-100 个字符。')
  const version = String(raw.version || '').trim()
  if (!VERSION_PATTERN.test(version))
    throw new Error('插件 version 必须是有效的语义版本，例如 1.0.0。')
  const entry = normalizeRelativePath(raw.entry, 'entry')
  if (!ALLOWED_ENTRY_EXTENSIONS.has(extname(entry).toLowerCase()))
    throw new Error('插件 entry 仅支持 .js、.mjs 或 .cjs 文件。')
  const tools = (Array.isArray(raw.tools) ? raw.tools : []).map(normalizeTool)
  if (tools.length === 0 || tools.length > 32) throw new Error('插件必须声明 1-32 个工具。')
  if (new Set(tools.map((tool) => tool.name)).size !== tools.length)
    throw new Error('插件工具名称不能重复。')
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
      throw new Error(`${MANIFEST_FILE} 必须是普通文件。`)
    if (info.size > MAX_MANIFEST_BYTES) throw new Error(`${MANIFEST_FILE} 超过 256 KB 限制。`)
    raw = JSON.parse(await readFile(manifestPath, 'utf8'))
  } catch (error) {
    if (error?.code === 'ENOENT') throw new Error(`目录中缺少 ${MANIFEST_FILE}。`)
    if (error instanceof SyntaxError) throw new Error(`${MANIFEST_FILE} 不是有效的 JSON。`)
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
      if (entry.isSymbolicLink()) throw new Error(`插件目录不能包含符号链接：${relativePath}`)
      if (entry.isDirectory()) {
        await visit(absolutePath)
        continue
      }
      if (!entry.isFile()) throw new Error(`插件目录包含不支持的文件类型：${relativePath}`)
      const info = await stat(absolutePath)
      fileCount += 1
      byteCount += info.size
      if (fileCount > MAX_PLUGIN_FILES) throw new Error(`插件文件数不能超过 ${MAX_PLUGIN_FILES}。`)
      if (byteCount > MAX_PLUGIN_BYTES) throw new Error('插件目录大小不能超过 20 MB。')
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
    description: '',
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
    throw new Error('插件返回了无法序列化的结果。')
  }
  if (Buffer.byteLength(serialized, 'utf8') > MAX_RESULT_BYTES)
    throw new Error('插件返回结果超过 1 MB 限制。')
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
      throw new Error('插件安装根目录必须是普通目录。')
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
  async ensureDefaultTools(toolIds, migrationKey) {
    const appConfig = await readJson(this.configPath, { toolMode: 'full' })
    if (appConfig[migrationKey]) return
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
          throw new Error('插件安装记录无效。')
        }
        const root = join(this.pluginRoot, id, String(record.version || ''))
        const manifest = await readManifest(root)
        if (manifest.id !== id || manifest.version !== record.version)
          throw new Error('插件清单与安装记录不一致。')
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
      if (TOOL_IDS.has(tool.name)) throw new Error(`工具名称 ${tool.name} 与内置工具冲突。`)
      for (const [pluginId, installed] of this.installed) {
        if (
          pluginId !== ignoredPluginId &&
          installed.manifest.tools.some((candidate) => candidate.name === tool.name)
        ) {
          throw new Error(`工具名称 ${tool.name} 已由插件 ${pluginId} 提供。`)
        }
      }
    }
  }

  // 检查插件包：解析清单/入口/能力，生成可安装的检查结果。
  async inspect(sourcePath) {
    const requestedPath = String(sourcePath || '').trim()
    if (!requestedPath) throw new Error('请选择插件目录。')
    const requestedRoot = resolve(requestedPath)
    const requestedInfo = await lstat(requestedRoot)
    if (requestedInfo.isSymbolicLink() || !requestedInfo.isDirectory()) {
      throw new Error('插件来源必须是不含符号链接的目录。')
    }
    const sourceRoot = await realpath(requestedRoot)
    const manifest = await readManifest(sourceRoot)
    const entryPath = await realpath(join(sourceRoot, manifest.entry))
    if (!isPathInside(sourceRoot, entryPath) || !(await stat(entryPath)).isFile())
      throw new Error('插件 entry 必须指向目录内的普通文件。')
    if (this.installed.has(manifest.id) || this.state.plugins[manifest.id])
      throw new Error(`插件 ${manifest.id} 已安装；当前版本不支持覆盖安装。`)
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
        '插件代码将在独立 Worker 中运行，但仍拥有当前系统用户可访问的本机文件和网络权限。',
        '第三方插件能力仅在“完全访问”执行模式下提供给 Agent。',
      ],
    }
  }

  // 安装已检查的插件：解包到插件目录并激活。
  async install(inspectionId) {
    const inspection = this.inspections.get(String(inspectionId || ''))
    if (!inspection || inspection.expiresAt < Date.now())
      throw new Error('插件检查结果已过期，请重新检查目录。')
    const { sourceRoot, manifest, scan } = inspection
    if (this.installed.has(manifest.id) || this.state.plugins[manifest.id])
      throw new Error(`插件 ${manifest.id} 已安装。`)
    this.#validateToolConflicts(manifest)
    if (this.changingPlugins.has(manifest.id)) throw new Error('插件正在变更，请稍后重试。')
    this.changingPlugins.add(manifest.id)
    try {
      const currentScan = await scanDirectory(sourceRoot)
      if (currentScan.digest !== scan.digest)
        throw new Error('插件目录在检查后发生了变化，请重新检查。')

      const pluginDirectory = join(this.pluginRoot, manifest.id)
      const destination = join(pluginDirectory, manifest.version)
      const stage = `${destination}.install-${randomUUID()}`
      await mkdir(pluginDirectory, { recursive: true })
      const pluginDirectoryInfo = await lstat(pluginDirectory)
      if (pluginDirectoryInfo.isSymbolicLink() || !pluginDirectoryInfo.isDirectory()) {
        throw new Error('插件安装目录必须是普通目录。')
      }
      try {
        await open(destination, 'wx')
          .then((handle) => handle.close())
          .catch((error) => {
            if (error?.code === 'EISDIR' || error?.code === 'EEXIST')
              throw new Error(`插件 ${manifest.id}@${manifest.version} 已存在。`)
            throw error
          })
        await rm(destination, { force: true })
        await cp(sourceRoot, stage, { recursive: true, errorOnExist: true, force: false })
        const copiedScan = await scanDirectory(stage)
        if (copiedScan.digest !== scan.digest)
          throw new Error('插件目录在复制期间发生了变化，请重新检查。')
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
    if (!entryCode.trim()) throw new Error('插件入口代码不能为空。')
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
      throw new Error(`插件 ${manifest.id} 已安装，不能覆盖。`)
    }

    const extraFiles = Array.isArray(input.files) ? input.files : []
    if (extraFiles.length > 64) throw new Error('插件附加文件不能超过 64 个。')
    const normalizedFiles = []
    const seenPaths = new Set([MANIFEST_FILE, manifest.entry])
    for (const [index, file] of extraFiles.entries()) {
      if (!file || typeof file !== 'object' || Array.isArray(file)) {
        throw new Error(`files[${index}] 必须是对象。`)
      }
      const path = normalizeRelativePath(file.path, `files[${index}].path`).replace(/^\.\//, '')
      if (path === '.' || path.endsWith('/') || seenPaths.has(path)) {
        throw new Error(`插件附加文件路径重复或无效：${path}`)
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
    if (generatedBytes > MAX_PLUGIN_BYTES) throw new Error('插件生成内容不能超过 20 MB。')

    const canonicalDataDir = await realpath(this.dataDir)
    const sourceRoot = join(canonicalDataDir, 'plugin-sources')
    await mkdir(sourceRoot).catch((error) => {
      if (error?.code !== 'EEXIST') throw error
    })
    const sourceRootInfo = await lstat(sourceRoot)
    if (sourceRootInfo.isSymbolicLink() || !sourceRootInfo.isDirectory()) {
      throw new Error('全局插件源码目录必须是普通目录，不能使用符号链接。')
    }
    const canonicalSourceRoot = await realpath(sourceRoot)
    if (!isPathInside(canonicalDataDir, canonicalSourceRoot)) {
      throw new Error('全局插件源码目录不能指向 Pisper Agent 目录之外。')
    }

    const destination = join(canonicalSourceRoot, manifest.id)
    try {
      await mkdir(destination)
    } catch (error) {
      if (error?.code === 'EEXIST')
        throw new Error(`插件源码目录 ${destination} 已存在，不能覆盖。`)
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
    if (!plugin) throw new Error(`插件 ${pluginId} 不存在。`)
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
      throw new Error(`插件 ${pluginId} 不提供能力 ${toolName}。`)
    const enabledTools = enabled
      ? [...new Set([...current.enabledTools, toolName])]
      : current.enabledTools.filter((name) => name !== toolName)
    return await this.saveState({ enabledTools })
  }

  async uninstall(pluginId) {
    const installed = this.installed.get(pluginId)
    if (!installed) throw new Error(`本地插件 ${pluginId} 不存在。`)
    if (this.changingPlugins.has(pluginId)) throw new Error('插件正在变更，请稍后重试。')
    this.changingPlugins.add(pluginId)
    const pluginDirectory = join(this.pluginRoot, pluginId)
    const quarantine = join(this.pluginRoot, `.uninstall-${pluginId}-${randomUUID()}`)
    let quarantined = false
    try {
      if ((this.activeExecutions.get(pluginId) || 0) > 0)
        throw new Error('插件正在执行，暂时无法卸载。')
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

  // 按执行模式过滤出的启用工具列表。
  enabledTools(appConfig, executionMode) {
    const builtIns = toolsFromConfig(appConfig)
    if (executionMode !== 'full-access') return builtIns
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
    if (this.disposed) throw new Error('插件服务已停止。')
    if (this.changingPlugins.has(pluginId)) throw new Error('插件正在变更，请稍后重试。')
    this.activeExecutions.set(pluginId, (this.activeExecutions.get(pluginId) || 0) + 1)
    let worker
    try {
      await mkdir(context.dataDir, { recursive: true })
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
          finish(rejectResult, new Error('插件执行已取消。'))
        }
        const timer = setTimeout(() => {
          void worker.terminate()
          finish(rejectResult, new Error('插件执行超过 120 秒，已终止。'))
        }, EXECUTION_TIMEOUT_MS)
        worker.on('message', (message) => {
          if (message?.type === 'result') finish(resolveResult, normalizeToolResult(message.result))
          else if (message?.type === 'error')
            finish(rejectResult, new Error(message.error?.message || '插件执行失败。'))
        })
        worker.on('error', (error) => finish(rejectResult, error))
        worker.on('exit', (code) => {
          if (code !== 0) finish(rejectResult, new Error(`插件 Worker 异常退出（code ${code}）。`))
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
