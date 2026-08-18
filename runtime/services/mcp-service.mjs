// MCP 服务：管理 MCP 服务器配置（stdio/SSE/streamable HTTP），
// 生成工具定义注入会话，并把工具调用转发到远端 MCP 服务器。
// SDK 懒加载，只有配置了 MCP 服务器时才引入 @modelcontextprotocol/sdk。
import { createHash, randomUUID } from 'node:crypto'
import { stat } from 'node:fs/promises'
import { isAbsolute, resolve } from 'node:path'
import {
  DEFAULT_MAX_BYTES,
  DEFAULT_MAX_LINES,
  defineTool,
  formatSize,
  truncateHead,
} from '../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const MCP_STATE_VERSION = 1
const MAX_MCP_SERVERS = 32
const MAX_MCP_TOOLS_PER_SERVER = 500
const MAX_MCP_SCHEMA_BYTES = 256 * 1024
const MAX_MCP_SCHEMA_BYTES_PER_SERVER = 4 * 1024 * 1024
const MAX_MCP_CALLS = 100
const DEFAULT_MCP_TIMEOUT_MS = 60_000
const MCP_CONNECT_TIMEOUT_MS = 10_000
const MAX_MCP_TIMEOUT_MS = 10 * 60_000
const MAX_MCP_IMAGE_BYTES = 10 * 1024 * 1024
const STDERR_ATTACHED = Symbol('pisperMcpStderrAttached')

// 懒加载 MCP SDK：四种传输模块一次性导入并缓存。
let mcpSdkPromise
function loadMcpSdk() {
  mcpSdkPromise ||= Promise.all([
    import('@modelcontextprotocol/sdk/client/index.js'),
    import('@modelcontextprotocol/sdk/client/sse.js'),
    import('@modelcontextprotocol/sdk/client/stdio.js'),
    import('@modelcontextprotocol/sdk/client/streamableHttp.js'),
  ]).then(([client, sse, stdio, streamableHttp]) => ({
    Client: client.Client,
    SSEClientTransport: sse.SSEClientTransport,
    StdioClientTransport: stdio.StdioClientTransport,
    getDefaultEnvironment: stdio.getDefaultEnvironment,
    StreamableHTTPClientTransport: streamableHttp.StreamableHTTPClientTransport,
  }))
  return mcpSdkPromise
}

function clone(value) {
  return JSON.parse(JSON.stringify(value))
}

function nowIso() {
  return new Date().toISOString()
}

function safeString(value, limit = 2_000) {
  return String(value || '')
    .trim()
    .slice(0, limit)
}

function safeRecord(value, { maxEntries = 50, maxValueChars = 8_000 } = {}) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
  return Object.fromEntries(
    Object.entries(value)
      .slice(0, maxEntries)
      .map(([key, item]) => [safeString(key, 200), safeString(item, maxValueChars)]),
  )
}

function safeArgs(value) {
  return Array.isArray(value) ? value.slice(0, 100).map((item) => safeString(item, 4_000)) : []
}

function safeTimeout(value) {
  const number = Number(value)
  if (!Number.isFinite(number)) return DEFAULT_MCP_TIMEOUT_MS
  return Math.max(1_000, Math.min(MAX_MCP_TIMEOUT_MS, Math.round(number)))
}

function hasControlCharacters(value) {
  return [...String(value || '')].some((character) => {
    const code = character.charCodeAt(0)
    return code <= 31 || code === 127
  })
}

function pathLikeCommand(value) {
  return isAbsolute(value) || /^[a-z]:[\\/]/i.test(value) || /[\\/]/.test(value)
}

function normalizeTool(tool) {
  const name = typeof tool?.name === 'string' ? tool.name : ''
  if (
    !tool ||
    typeof tool !== 'object' ||
    !name.trim() ||
    name.length > 300 ||
    ['__proto__', 'constructor', 'prototype'].includes(name)
  )
    return null
  let schema = { type: 'object', properties: {} }
  if (
    tool.inputSchema &&
    typeof tool.inputSchema === 'object' &&
    !Array.isArray(tool.inputSchema)
  ) {
    try {
      const serialized = JSON.stringify(tool.inputSchema)
      if (Buffer.byteLength(serialized, 'utf8') > MAX_MCP_SCHEMA_BYTES) return null
      schema = JSON.parse(serialized)
    } catch {
      return null
    }
  }
  if (schema.type !== 'object') schema.type = 'object'
  const annotations =
    tool.annotations && typeof tool.annotations === 'object'
      ? Object.fromEntries(
          ['readOnlyHint', 'destructiveHint', 'idempotentHint', 'openWorldHint']
            .filter((key) => typeof tool.annotations[key] === 'boolean')
            .map((key) => [key, tool.annotations[key]]),
        )
      : {}
  return {
    name,
    title: safeString(tool.title, 300),
    description: safeString(tool.description, 8_000),
    inputSchema: schema,
    annotations,
  }
}

function normalizeTools(values) {
  const tools = []
  let schemaBytes = 0
  for (const value of (Array.isArray(values) ? values : []).slice(0, MAX_MCP_TOOLS_PER_SERVER)) {
    const tool = normalizeTool(value)
    if (!tool) continue
    const bytes = Buffer.byteLength(JSON.stringify(tool.inputSchema), 'utf8')
    if (schemaBytes + bytes > MAX_MCP_SCHEMA_BYTES_PER_SERVER) continue
    schemaBytes += bytes
    tools.push(tool)
  }
  return tools
}

function normalizeToolStates(...values) {
  const result = {}
  for (const source of values) {
    if (!source || typeof source !== 'object' || Array.isArray(source)) continue
    for (const [name, enabled] of Object.entries(source)) {
      if (Object.keys(result).length >= MAX_MCP_TOOLS_PER_SERVER) break
      if (typeof enabled === 'boolean' && !['__proto__', 'constructor', 'prototype'].includes(name))
        result[name] = enabled
    }
  }
  return result
}

function pruneToolStates(toolStates, tools) {
  const result = {}
  for (const tool of tools) {
    if (typeof toolStates?.[tool.name] === 'boolean') result[tool.name] = toolStates[tool.name]
  }
  return result
}

function connectionFingerprint(server) {
  return JSON.stringify(
    server?.transport === 'stdio'
      ? {
          transport: 'stdio',
          command: server.command,
          args: server.args || [],
          cwd: server.cwd || '',
          env: server.env || {},
        }
      : {
          transport: server?.transport || 'http',
          url: server?.url || '',
          headers: server?.headers || {},
        },
  )
}

function normalizeServer(value, existing = {}) {
  const requestedTransport = value?.transport || value?.type
  const normalizedTransport = requestedTransport === 'streamable-http' ? 'http' : requestedTransport
  const transport = ['stdio', 'http', 'sse'].includes(normalizedTransport)
    ? normalizedTransport
    : value?.command
      ? 'stdio'
      : value?.url
        ? 'http'
        : existing.transport || 'http'
  const server = {
    id: safeString(existing.id || value?.id || randomUUID(), 100),
    name: safeString(value?.name || existing.name || 'MCP Server', 120),
    transport,
    enabled: typeof value?.enabled === 'boolean' ? value.enabled : existing.enabled !== false,
    requestTimeoutMs: safeTimeout(value?.requestTimeoutMs ?? existing.requestTimeoutMs),
    toolStates: normalizeToolStates(existing.toolStates, value?.toolStates),
    tools: normalizeTools(existing.tools),
    createdAt: existing.createdAt || nowIso(),
    updatedAt: nowIso(),
  }
  server.toolStates = pruneToolStates(server.toolStates, server.tools)
  if (transport === 'stdio') {
    server.command = safeString(value?.command ?? existing.command, 2_000)
    if (!server.command) throw new Error('stdio MCP 服务需要 command。')
    server.args = safeArgs(value?.args ?? existing.args)
    server.cwd = safeString(value?.cwd ?? existing.cwd, 4_000)
    server.env = safeRecord(value?.env ?? existing.env)
    delete server.url
    delete server.headers
  } else {
    const url = new URL(safeString(value?.url ?? existing.url, 8_000))
    if (!['http:', 'https:'].includes(url.protocol))
      throw new Error('MCP URL 仅支持 http 或 https。')
    server.url = url.toString()
    server.headers = safeRecord(value?.headers ?? existing.headers)
    delete server.command
    delete server.args
    delete server.cwd
    delete server.env
  }
  if (existing.id && connectionFingerprint(existing) !== connectionFingerprint(server)) {
    server.tools = []
    server.toolStates = {}
  }
  return server
}

function normalizeCall(value) {
  if (!value || typeof value !== 'object') return null
  const duration = Number(value.durationMs)
  return {
    id: safeString(value.id || randomUUID(), 100),
    serviceId: safeString(value.serviceId, 100),
    serviceName: safeString(value.serviceName, 120),
    toolName: safeString(value.toolName, 300),
    piToolName: safeString(value.piToolName, 100),
    timestamp: safeString(value.timestamp || nowIso(), 100),
    durationMs: Number.isFinite(duration) ? Math.max(0, Math.round(duration)) : 0,
    status: value.status === 'error' ? 'error' : 'ok',
    ...(value.error ? { error: safeString(redactSecrets(value.error), 2_000) } : {}),
  }
}

function normalizeState(input) {
  const source = Array.isArray(input?.servers) ? input.servers : []
  const servers = []
  for (const value of source.slice(0, MAX_MCP_SERVERS)) {
    try {
      servers.push(normalizeServer(value, value))
    } catch {}
  }
  const calls = (Array.isArray(input?.calls) ? input.calls : [])
    .slice(0, MAX_MCP_CALLS)
    .map(normalizeCall)
    .filter(Boolean)
  return { version: MCP_STATE_VERSION, servers, calls }
}

function slug(value, limit = 20) {
  const result = String(value || '')
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^_|_$/g, '')
  return (result || 'tool').slice(0, limit)
}

function piToolName(server, tool) {
  const hash = createHash('sha256').update(`${server.id}:${tool.name}`).digest('hex').slice(0, 8)
  return `mcp_${slug(server.name, 16)}_${slug(tool.name, 30)}_${hash}`.slice(0, 64)
}

function toolRisk(tool) {
  if (tool.annotations?.destructiveHint) return 'high'
  if (tool.annotations?.readOnlyHint) return 'low'
  return 'medium'
}

function statusFromError(error) {
  const message = error instanceof Error ? error.message : String(error)
  return /401|403|unauthori[sz]ed|forbidden|auth/i.test(message) ? 'unauthorized' : 'offline'
}

function publicStatus(status) {
  return ['online', 'connecting', 'offline', 'unauthorized', 'disabled'].includes(status)
    ? status
    : 'offline'
}

function formatEndpoint(server) {
  if (server.transport === 'stdio') return [server.command, ...(server.args || [])].join(' ')
  return server.url
}

function redactSecrets(value) {
  return String(value || '')
    .replace(/(bearer\s+)[^\s,;]+/gi, '$1***')
    .replace(/([?&](?:token|key|secret|password|auth)[^=]*=)[^&#\s]*/gi, '$1***')
    .replace(/(\/\/)[^/@\s]+@/g, '$1***@')
    .replace(/((?:api[_-]?key|token|secret|password)\s*[:=]\s*)[^\s,;]+/gi, '$1***')
}

function publicEndpoint(server) {
  if (server.transport === 'stdio') {
    let hideNext = false
    const args = (server.args || []).map((arg) => {
      if (hideNext) {
        hideNext = false
        return '***'
      }
      if (/^--?(?:token|key|secret|password|auth)$/i.test(arg)) {
        hideNext = true
        return arg
      }
      return redactSecrets(arg)
    })
    const quote = (value) => (/\s/.test(value) ? `"${value.replaceAll('"', '\\"')}"` : value)
    return [quote(redactSecrets(server.command)), ...args.map(quote)].join(' ')
  }
  return redactSecrets(server.url)
}

function authInfo(server) {
  if (server.transport === 'stdio') {
    const count = Object.keys(server.env || {}).length
    return { auth: count ? 'environment' : 'local', authCount: count }
  }
  const count = Object.keys(server.headers || {}).length
  return { auth: count ? 'headers' : 'none', authCount: count }
}

function combineSignal(signal, timeoutMs) {
  const timeout = AbortSignal.timeout(timeoutMs)
  return signal ? AbortSignal.any([signal, timeout]) : timeout
}

function utf8Prefix(value, maxBytes) {
  const buffer = Buffer.from(value, 'utf8')
  if (buffer.length <= maxBytes) return value
  let end = maxBytes
  while (end > 0 && (buffer[end] & 0xc0) === 0x80) end -= 1
  return buffer.subarray(0, end).toString('utf8')
}

function boundedStructuredContent(value) {
  if (!value || typeof value !== 'object') return value
  try {
    const serialized = JSON.stringify(value)
    if (Buffer.byteLength(serialized, 'utf8') <= DEFAULT_MAX_BYTES) return value
    return { truncated: true, preview: utf8Prefix(serialized, DEFAULT_MAX_BYTES) }
  } catch {
    return { unavailable: true }
  }
}

function normalizedProgress(progress) {
  const result = {}
  if (Number.isFinite(Number(progress?.progress))) result.progress = Number(progress.progress)
  if (Number.isFinite(Number(progress?.total))) result.total = Number(progress.total)
  if (progress?.message) result.message = safeString(redactSecrets(progress.message), 500)
  return result
}

function progressText(progress) {
  const total = Number(progress?.total)
  const current = Number(progress?.progress)
  if (Number.isFinite(current) && Number.isFinite(total) && total > 0)
    return `MCP progress: ${current}/${total}`
  if (Number.isFinite(current)) return `MCP progress: ${current}`
  return 'MCP tool is working'
}

function truncatedAgentContent(value, images = []) {
  const fullText =
    value === '' || value == null ? '(MCP tool returned no text content.)' : String(value)
  const truncated = truncateHead(fullText, {
    maxLines: DEFAULT_MAX_LINES,
    maxBytes: DEFAULT_MAX_BYTES,
  })
  let text = truncated.content
  if (truncated.firstLineExceedsLimit) text = utf8Prefix(fullText, DEFAULT_MAX_BYTES - 160)
  if (truncated.truncated) {
    text += `\n\n[Output truncated: ${truncated.outputLines}/${truncated.totalLines} lines, ${formatSize(truncated.outputBytes)}/${formatSize(truncated.totalBytes)}.]`
  }
  return {
    content: [{ type: 'text', text }, ...images],
    text: fullText,
    truncated: truncated.truncated,
  }
}

function mcpContentToAgent(result) {
  if (result && typeof result === 'object' && 'toolResult' in result) {
    const text =
      typeof result.toolResult === 'string'
        ? result.toolResult
        : (JSON.stringify(result.toolResult, null, 2) ?? String(result.toolResult ?? ''))
    return truncatedAgentContent(text)
  }
  const textParts = []
  const images = []
  let imageBytes = 0
  for (const item of Array.isArray(result?.content) ? result.content : []) {
    if (item?.type === 'text') textParts.push(item.text || '')
    else if (item?.type === 'image') {
      const bytes = Buffer.byteLength(item.data || '', 'base64')
      if (bytes > 0 && imageBytes + bytes <= MAX_MCP_IMAGE_BYTES) {
        imageBytes += bytes
        images.push({ type: 'image', data: item.data, mimeType: item.mimeType })
      } else {
        textParts.push(
          `[MCP image omitted: image payload exceeds the ${formatSize(MAX_MCP_IMAGE_BYTES)} aggregate limit]`,
        )
      }
    } else if (item?.type === 'resource') {
      if (typeof item.resource?.text === 'string')
        textParts.push(`[Resource ${item.resource.uri}]\n${item.resource.text}`)
      else textParts.push(`[Binary resource ${item.resource?.uri || 'unknown'} omitted]`)
    } else if (item?.type === 'resource_link') {
      textParts.push(`[Resource link] ${item.name || item.uri}: ${item.uri}`)
    } else if (item?.type === 'audio') {
      textParts.push(`[Audio content omitted: ${item.mimeType || 'unknown type'}]`)
    }
  }
  if (result?.structuredContent && typeof result.structuredContent === 'object') {
    textParts.push(`Structured content:\n${JSON.stringify(result.structuredContent, null, 2)}`)
  }
  return truncatedAgentContent(textParts.filter(Boolean).join('\n\n'), images)
}

function unwrapMcpServerConfig(value) {
  const collection = value?.mcpServers || value?.servers
  if (!collection || typeof collection !== 'object' || Array.isArray(collection)) return value
  const entries = Object.entries(collection)
  if (entries.length !== 1) throw new Error('一次只能添加一个 MCP 服务。')
  const [name, config] = entries[0]
  return { ...(config || {}), name: config?.name || name }
}

export function parseMcpServerInput(input, cwd = process.cwd()) {
  if (input && typeof input === 'object' && !Array.isArray(input) && !input.spec)
    return normalizeServer(unwrapMcpServerConfig(input))
  const rawSpec = String(input?.spec ?? input ?? '')
  if (rawSpec.length > 12_000) throw new Error('MCP 配置过长。')
  const spec = rawSpec.trim()
  if (!spec) throw new Error('请输入 MCP URL、stdio 命令或 JSON 配置。')
  if (spec.startsWith('{')) {
    let parsed
    try {
      parsed = JSON.parse(spec)
    } catch {
      throw new Error('MCP JSON 配置格式无效。')
    }
    return normalizeServer(unwrapMcpServerConfig(parsed))
  }
  if (/^https?:\/\//i.test(spec)) {
    const url = new URL(spec)
    return normalizeServer({ name: url.hostname, transport: 'http', url: spec })
  }
  const parts = []
  const matcher = /"((?:\\.|[^"\\])*)"|'([^']*)'|([^\s]+)/g
  let match
  while ((match = matcher.exec(spec)))
    parts.push((match[1] ?? match[2] ?? match[3] ?? '').replace(/\\"/g, '"'))
  if (!parts.length) throw new Error('stdio MCP 命令不能为空。')
  return normalizeServer({
    name: slug(
      parts[0]
        .split(/[\\/]/)
        .at(-1)
        ?.replace(/\.(?:exe|cmd|bat)$/i, '') || 'MCP Server',
      40,
    ),
    transport: 'stdio',
    command: parts[0],
    args: parts.slice(1),
    cwd,
  })
}

export class McpService {
  constructor({ path, cwd, createClient, createTransport } = {}) {
    // 依赖注入：createClient/createTransport 便于测试替换真实 MCP SDK。
    this.path = path
    this.cwd = cwd || process.cwd()
    this.createClient =
      createClient ||
      (async (server, handlers) => {
        const { Client } = await loadMcpSdk()
        return new Client(
          { name: 'pisper', version: '0.0.0' },
          {
            capabilities: {},
            listChanged: { tools: { onChanged: handlers.onToolsChanged } },
          },
        )
      })
    this.createTransport =
      createTransport ||
      (async (server, onStderr) => {
        const {
          SSEClientTransport,
          StdioClientTransport,
          StreamableHTTPClientTransport,
          getDefaultEnvironment,
        } = await loadMcpSdk()
        if (server.transport === 'stdio') {
          const transport = new StdioClientTransport({
            command: server.command,
            args: server.args,
            cwd: server.cwd ? resolve(server.cwd) : this.cwd,
            env: { ...getDefaultEnvironment(), ...(server.env || {}) },
            stderr: 'pipe',
          })
          if (transport.stderr?.on) {
            transport[STDERR_ATTACHED] = true
            transport.stderr.on('data', (chunk) => onStderr(safeString(chunk, 2_000)))
          }
          return transport
        }
        const headers = server.headers || {}
        if (server.transport === 'sse') {
          return new SSEClientTransport(new URL(server.url), {
            requestInit: { headers },
            eventSourceInit: {
              fetch: (url, init) => {
                const merged = new Headers(init?.headers)
                for (const [name, value] of Object.entries(headers)) merged.set(name, value)
                return fetch(url, { ...init, headers: merged })
              },
            },
          })
        }
        return new StreamableHTTPClientTransport(new URL(server.url), { requestInit: { headers } })
      })
    this.state = { version: MCP_STATE_VERSION, servers: [], calls: [] }
    this.connections = new Map()
    this.calls = this.state.calls
    this.write = Promise.resolve()
  }

  async init() {
    // 加载服务器配置并按需预连。
    this.state = normalizeState(
      await readJson(this.path, { version: MCP_STATE_VERSION, servers: [], calls: [] }),
    )
    this.calls = this.state.calls
  }

  // 校验服务器配置：命令/URL/环境变量安全性与格式。
  async validateServer(server) {
    if (server.transport !== 'stdio') return server
    if (hasControlCharacters(server.command))
      throw new Error(
        'stdio MCP command contains invalid control characters. Use a structured command field without shell or JavaScript escaping.',
      )
    if ((server.args || []).some(hasControlCharacters))
      throw new Error('stdio MCP args contain invalid control characters.')
    if (
      Object.entries(server.env || {}).some(
        ([key, value]) => hasControlCharacters(key) || hasControlCharacters(value),
      )
    ) {
      throw new Error('stdio MCP environment variables contain invalid control characters.')
    }

    let workingDirectory = this.cwd
    if (server.cwd) {
      workingDirectory = resolve(this.cwd, server.cwd)
      const cwdInfo = await stat(workingDirectory).catch(() => null)
      if (!cwdInfo?.isDirectory())
        throw new Error(`stdio MCP working directory does not exist: ${workingDirectory}`)
      server.cwd = workingDirectory
    }
    if (!pathLikeCommand(server.command)) return server
    const commandPath =
      isAbsolute(server.command) || /^[a-z]:[\\/]/i.test(server.command)
        ? resolve(server.command)
        : resolve(workingDirectory, server.command)
    const commandInfo = await stat(commandPath).catch(() => null)
    if (!commandInfo?.isFile())
      throw new Error(`stdio MCP executable does not exist: ${commandPath}`)
    server.command = commandPath
    return server
  }

  save() {
    const snapshot = clone(this.state)
    this.write = this.write.catch(() => {}).then(() => writeJsonAtomic(this.path, snapshot))
    return this.write
  }

  getServer(id) {
    return this.state.servers.find((server) => server.id === id) || null
  }

  connectionFor(id) {
    if (!this.connections.has(id)) {
      this.connections.set(id, {
        status: 'offline',
        client: null,
        transport: null,
        connecting: null,
        error: '',
        stderr: '',
        latencyMs: null,
        connectedAt: '',
        lastPingAt: '',
        serverVersion: null,
        capabilities: null,
        nextRetryAt: 0,
      })
    }
    return this.connections.get(id)
  }

  async closeConnection(id) {
    const connection = this.connections.get(id)
    if (!connection) return
    connection.closing = true
    try {
      await connection.client?.close?.()
    } catch {}
    connection.client = null
    connection.transport = null
    connection.connecting = null
    connection.status = this.getServer(id)?.enabled === false ? 'disabled' : 'offline'
    connection.nextRetryAt = 0
    connection.closing = false
  }

  async listAllTools(client, server, signal) {
    const tools = []
    let cursor
    for (let page = 0; page < 20; page += 1) {
      const result = await client.listTools(cursor ? { cursor } : undefined, {
        signal,
        timeout: server.requestTimeoutMs,
        resetTimeoutOnProgress: true,
      })
      tools.push(...(result.tools || []))
      if (tools.length >= MAX_MCP_TOOLS_PER_SERVER) break
      cursor = result.nextCursor
      if (!cursor) break
    }
    return normalizeTools(tools)
  }

  // 确保服务器连接就绪（必要时重建）。
  async ensureConnected(id, { force = false, signal } = {}) {
    const server = this.getServer(id)
    if (!server) throw new Error('MCP 服务不存在。')
    if (!server.enabled) throw new Error('MCP 服务已禁用。')
    const connection = this.connectionFor(id)
    if (!force && connection.status === 'online' && connection.client) return connection
    if (!force && connection.connecting) return connection.connecting
    if (!force && connection.nextRetryAt > Date.now())
      throw new Error(connection.error || 'MCP 服务暂时离线，请稍后重试。')
    if (force) await this.closeConnection(id)

    connection.status = 'connecting'
    connection.error = ''
    connection.stderr = ''
    connection.connecting = (async () => {
      const startedAt = Date.now()
      const connectTimeoutMs = Math.min(server.requestTimeoutMs, MCP_CONNECT_TIMEOUT_MS)
      const requestSignal = combineSignal(signal, connectTimeoutMs)
      let client
      const handlers = {
        onToolsChanged: async (error, tools) => {
          if (client && connection.client !== client) return
          if (error) {
            connection.error = error instanceof Error ? error.message : String(error)
            return
          }
          try {
            server.tools = client
              ? await this.listAllTools(client, server, combineSignal(undefined, connectTimeoutMs))
              : normalizeTools(tools)
            server.toolStates = pruneToolStates(server.toolStates, server.tools)
            server.updatedAt = nowIso()
            await this.save()
          } catch (refreshError) {
            connection.error =
              refreshError instanceof Error ? refreshError.message : String(refreshError)
          }
        },
      }
      client = await this.createClient(server, handlers)
      const transport = await this.createTransport(server, (message) => {
        connection.stderr = message
      })
      connection.client = client
      connection.transport = transport
      client.onclose = () => {
        if (connection.closing || connection.client !== client) return
        connection.status = 'offline'
        connection.client = null
        connection.transport = null
      }
      client.onerror = (error) => {
        if (connection.client === client)
          connection.error = error instanceof Error ? error.message : String(error)
      }
      try {
        await client.connect(transport, { signal: requestSignal, timeout: connectTimeoutMs })
        if (transport.stderr?.on && !transport[STDERR_ATTACHED]) {
          transport[STDERR_ATTACHED] = true
          transport.stderr.on('data', (chunk) => {
            connection.stderr = safeString(chunk, 2_000)
          })
        }
        server.tools = await this.listAllTools(client, server, requestSignal)
        server.toolStates = pruneToolStates(server.toolStates, server.tools)
        server.updatedAt = nowIso()
        connection.status = 'online'
        connection.latencyMs = Date.now() - startedAt
        connection.connectedAt = nowIso()
        connection.lastPingAt = connection.connectedAt
        connection.serverVersion = client.getServerVersion?.() || null
        connection.capabilities = client.getServerCapabilities?.() || null
        connection.error = ''
        connection.nextRetryAt = 0
        await this.save()
        return connection
      } catch (error) {
        connection.status = statusFromError(error)
        connection.error = error instanceof Error ? error.message : String(error)
        connection.nextRetryAt = Date.now() + 30_000
        try {
          await client.close?.()
        } catch {}
        connection.client = null
        connection.transport = null
        throw error
      } finally {
        connection.connecting = null
      }
    })()
    return connection.connecting
  }

  async refreshAll({ force = false } = {}) {
    await Promise.allSettled(
      this.state.servers
        .filter((server) => server.enabled)
        .map((server) => this.ensureConnected(server.id, { force })),
    )
  }

  publicTool(server, tool) {
    const enabled = server.toolStates?.[tool.name] !== false
    return {
      serviceId: server.id,
      serviceName: server.name,
      name: tool.name,
      piName: piToolName(server, tool),
      title: tool.title || tool.name,
      description: safeString(tool.description || 'MCP tool', 500),
      enabled,
      serviceEnabled: server.enabled,
      available: server.enabled && enabled,
      risk: toolRisk(tool),
      annotations: tool.annotations || {},
    }
  }

  publicServer(server) {
    const connection = this.connectionFor(server.id)
    const status = server.enabled ? publicStatus(connection.status) : 'disabled'
    const auth = authInfo(server)
    return {
      id: server.id,
      name: server.name,
      transport: server.transport,
      endpoint: publicEndpoint(server),
      ...(server.transport === 'stdio'
        ? {
            command: redactSecrets(server.command),
            args: (server.args || []).map(redactSecrets),
            workingDirectory: resolve(server.cwd || this.cwd),
          }
        : { url: redactSecrets(server.url) }),
      enabled: server.enabled,
      status,
      error: safeString(redactSecrets(connection.error || connection.stderr || ''), 2_000),
      latencyMs: connection.latencyMs,
      connectedAt: connection.connectedAt,
      lastPingAt: connection.lastPingAt,
      ...auth,
      serverVersion: connection.serverVersion,
      capabilities: connection.capabilities,
      toolCount: server.tools.length,
      enabledToolCount: server.tools.filter((tool) => server.toolStates?.[tool.name] !== false)
        .length,
      tools: server.tools.map((tool) => this.publicTool(server, tool)),
      requestTimeoutMs: server.requestTimeoutMs,
    }
  }

  dashboard() {
    const services = this.state.servers.map((server) => this.publicServer(server))
    const tools = services.flatMap((server) => server.tools)
    const cutoff = Date.now() - 24 * 60 * 60 * 1000
    const recent = this.calls.filter((call) => new Date(call.timestamp).getTime() >= cutoff)
    const errors = recent.filter((call) => call.status === 'error').length
    return {
      services,
      tools,
      calls: this.calls.slice(0, 20),
      metrics: {
        totalServices: services.length,
        onlineServices: services.filter((server) => server.status === 'online').length,
        availableTools: tools.filter((tool) => tool.available).length,
        restrictedTools: tools.filter(
          (tool) => tool.available && (tool.risk === 'high' || tool.risk === '高风险'),
        ).length,
        errorRate: recent.length ? Math.round((errors / recent.length) * 1_000) / 10 : 0,
      },
    }
  }

  async getDashboard({ refresh = false } = {}) {
    if (refresh) await this.refreshAll({ force: true })
    return this.dashboard()
  }

  async add(input = {}) {
    if (this.state.servers.length >= MAX_MCP_SERVERS)
      throw new Error(`最多配置 ${MAX_MCP_SERVERS} 个 MCP 服务。`)
    const server = parseMcpServerInput(input, this.cwd)
    await this.validateServer(server)
    if (
      this.state.servers.some(
        (item) => item.name === server.name && formatEndpoint(item) === formatEndpoint(server),
      )
    ) {
      throw new Error('该 MCP 服务已存在。')
    }
    this.state.servers.push(server)
    await this.save()
    try {
      await this.ensureConnected(server.id)
    } catch {}
    return this.dashboard()
  }

  async update(id, input = {}) {
    const index = this.state.servers.findIndex((server) => server.id === id)
    if (index < 0) return null
    const previous = this.state.servers[index]
    const updated = normalizeServer(input, previous)
    await this.validateServer(updated)
    this.state.servers[index] = updated
    await this.closeConnection(id)
    await this.save()
    if (updated.enabled) {
      try {
        await this.ensureConnected(id)
      } catch {}
    }
    return this.dashboard()
  }

  async remove(id) {
    const index = this.state.servers.findIndex((server) => server.id === id)
    if (index < 0) return false
    await this.closeConnection(id)
    this.state.servers.splice(index, 1)
    this.connections.delete(id)
    await this.save()
    return true
  }

  async setToolEnabled(id, toolName, enabled) {
    const server = this.getServer(id)
    if (!server) return null
    if (!server.tools.some((tool) => tool.name === toolName)) return null
    server.toolStates ||= {}
    server.toolStates[toolName] = Boolean(enabled)
    server.updatedAt = nowIso()
    await this.save()
    return this.dashboard()
  }

  async test(id, { signal } = {}) {
    const startedAt = Date.now()
    const connection = await this.ensureConnected(id, { force: true, signal })
    await connection.client.ping({
      signal: combineSignal(signal, this.getServer(id).requestTimeoutMs),
      timeout: this.getServer(id).requestTimeoutMs,
      resetTimeoutOnProgress: true,
    })
    connection.latencyMs = Date.now() - startedAt
    connection.lastPingAt = nowIso()
    connection.status = 'online'
    return this.dashboard()
  }

  recordCall(call) {
    const normalized = normalizeCall(call)
    if (!normalized) return
    this.calls.unshift(normalized)
    if (this.calls.length > MAX_MCP_CALLS) this.calls.length = MAX_MCP_CALLS
    void this.save().catch(() => {})
  }

  // 工具 → 定义转换：注入工具描述与 schema，供会话装配。
  createToolDefinition(server, tool) {
    const name = piToolName(server, tool)
    const service = this
    return defineTool({
      name,
      label: `MCP · ${server.name} · ${tool.title || tool.name}`,
      description: `${tool.description || tool.name}\nRemote MCP server: ${server.name}. Remote tool name: ${tool.name}.`,
      promptSnippet: `Call ${tool.name} on the ${server.name} MCP server`,
      promptGuidelines: [
        `Use ${name} only when the ${server.name} MCP capability is relevant to the user's task.`,
      ],
      parameters: Type.Unsafe(tool.inputSchema || { type: 'object', properties: {} }),
      async execute(_toolCallId, params, signal, onUpdate) {
        const startedAt = Date.now()
        const timestamp = nowIso()
        try {
          const currentServer = service.getServer(server.id)
          if (!currentServer?.enabled || currentServer.toolStates?.[tool.name] === false)
            throw new Error('该 MCP 工具当前已禁用。')
          const currentConnection = service.connectionFor(server.id)
          const connection = await service.ensureConnected(server.id, {
            signal,
            force: !currentConnection.client,
          })
          const result = await connection.client.callTool(
            { name: tool.name, arguments: params || {} },
            undefined,
            {
              signal,
              timeout: currentServer.requestTimeoutMs,
              resetTimeoutOnProgress: true,
              onprogress: (progress) =>
                onUpdate?.({
                  content: [{ type: 'text', text: progressText(progress) }],
                  details: {
                    serviceId: server.id,
                    toolName: tool.name,
                    progress: normalizedProgress(progress),
                  },
                }),
            },
          )
          const converted = mcpContentToAgent(result)
          if (result?.isError)
            throw new Error(converted.content[0]?.text || 'MCP tool returned an error.')
          service.recordCall({
            id: randomUUID(),
            serviceId: server.id,
            serviceName: server.name,
            toolName: tool.name,
            piToolName: name,
            timestamp,
            durationMs: Date.now() - startedAt,
            status: 'ok',
          })
          return {
            content: converted.content,
            details: {
              serviceId: server.id,
              serviceName: server.name,
              toolName: tool.name,
              piToolName: name,
              durationMs: Date.now() - startedAt,
              truncated: converted.truncated,
              structuredContent: boundedStructuredContent(result?.structuredContent),
            },
          }
        } catch (error) {
          service.recordCall({
            id: randomUUID(),
            serviceId: server.id,
            serviceName: server.name,
            toolName: tool.name,
            piToolName: name,
            timestamp,
            durationMs: Date.now() - startedAt,
            status: 'error',
            error: error instanceof Error ? error.message : String(error),
          })
          throw error
        }
      },
    })
  }

  // 汇总全部已启用工具的 MCP 工具定义（会话运行时装配时调用）。
  async createToolDefinitions() {
    await this.refreshAll()
    const definitions = []
    for (const server of this.state.servers) {
      if (!server.enabled) continue
      for (const tool of server.tools) {
        if (server.toolStates?.[tool.name] === false) continue
        definitions.push(this.createToolDefinition(server, tool))
      }
    }
    return definitions
  }

  getToolRisk(name) {
    for (const server of this.state.servers) {
      const tool = server.tools.find((item) => piToolName(server, item) === name)
      if (tool) return tool.annotations?.destructiveHint ? 'high' : 'medium'
    }
    return null
  }

  async dispose() {
    await Promise.allSettled([...this.connections.keys()].map((id) => this.closeConnection(id)))
    this.connections.clear()
    await this.write.catch(() => {})
  }
}
