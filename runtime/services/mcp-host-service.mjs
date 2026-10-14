// Pisper 对外 MCP 服务：用户开启后才在本机回环地址监听，凭独立令牌鉴权，
// 并与 Web/TUI 共用同一个 AgentRuntimeService 和会话权限。
import { randomBytes, timingSafeEqual } from 'node:crypto'
import { createServer } from 'node:http'
import { join, resolve } from 'node:path'
import { readJson, writeJsonAtomic } from '../storage/json-file.mjs'

const STATE_VERSION = 1
const MAX_BODY_BYTES = 256 * 1024
const MAX_CONCURRENT_REQUESTS = 16
let sdkPromise

function loadSdk() {
  sdkPromise ||= Promise.all([
    import('@modelcontextprotocol/sdk/server/mcp.js'),
    import('@modelcontextprotocol/sdk/server/streamableHttp.js'),
    import('./mcp-host-tools.mjs'),
  ])
    .then(([mcp, http, tools]) => ({
      McpServer: mcp.McpServer,
      StreamableHTTPServerTransport: http.StreamableHTTPServerTransport,
      registerMcpHostTools: tools.registerMcpHostTools,
      abortMcpHostRuns: tools.abortMcpHostRuns,
    }))
    .catch((error) => {
      sdkPromise = null
      throw error
    })
  return sdkPromise
}

function safeToken(value) {
  return typeof value === 'string' && /^[A-Za-z0-9_-]{43,128}$/.test(value) ? value : ''
}

function secureEqual(left, right) {
  const a = Buffer.from(String(left || ''))
  const b = Buffer.from(String(right || ''))
  return a.length === b.length && timingSafeEqual(a, b)
}

function bearer(header) {
  const match = /^Bearer ([A-Za-z0-9_-]{43,128})$/.exec(String(header || ''))
  return match?.[1] || ''
}

function sendError(res, status, code, message) {
  if (res.headersSent || res.writableEnded || res.destroyed) return
  const body = `${JSON.stringify({ error: message, code })}\n`
  res.writeHead(status, {
    'Cache-Control': 'no-store',
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  })
  res.end(body)
}

async function readBody(req) {
  const size = Number(req.headers['content-length'])
  if (Number.isFinite(size) && size > MAX_BODY_BYTES) {
    const error = new Error('MCP request body is too large.')
    error.status = 413
    throw error
  }
  const parts = []
  let total = 0
  for await (const chunk of req) {
    total += chunk.length
    if (total > MAX_BODY_BYTES) {
      const error = new Error('MCP request body is too large.')
      error.status = 413
      throw error
    }
    parts.push(chunk)
  }
  try {
    return JSON.parse(Buffer.concat(parts).toString('utf8'))
  } catch {
    const error = new Error('MCP request body must be valid JSON.')
    error.status = 400
    throw error
  }
}

/** 用户开关与监听生命周期；close 不改变持久化开关。 */
export class McpHostService {
  constructor({ dataDir, getRuntime, port = 5175, host = '127.0.0.1' } = {}) {
    if (!dataDir) throw new Error('MCP host dataDir is required.')
    if (typeof getRuntime !== 'function') throw new Error('MCP host getRuntime is required.')
    if (host !== '127.0.0.1') throw new Error('MCP host must bind to 127.0.0.1.')
    if (!Number.isInteger(port) || port < 0 || port > 65535) throw new Error('Invalid MCP port.')
    this.path = join(resolve(dataDir), 'pisper-mcp-host.json')
    this.getRuntime = getRuntime
    this.host = host
    this.port = port
    this.state = { version: STATE_VERSION, enabled: false, token: '' }
    this.loaded = null
    this.server = null
    this.transports = new Set()
    this.runs = new Map()
    this.activeRequests = 0
    this.error = ''
    this.transition = Promise.resolve()
    this.closed = false
  }

  async load() {
    this.loaded ||= (async () => {
      const value = await readJson(this.path, this.state)
      // 无效状态一律关闭，避免损坏配置文件意外开放监听。
      this.state = {
        version: STATE_VERSION,
        enabled:
          value?.version === STATE_VERSION &&
          value.enabled === true &&
          Boolean(safeToken(value.token)),
        token: safeToken(value?.token),
      }
    })()
    return this.loaded
  }

  status() {
    const address = this.server?.address()
    const port = typeof address === 'object' && address ? address.port : this.port
    return {
      enabled: this.state.enabled,
      listening: Boolean(this.server?.listening),
      host: this.host,
      port,
      url: this.server?.listening ? `http://${this.host}:${port}/mcp` : '',
      error: this.error || null,
    }
  }

  getCredentials() {
    const status = this.status()
    if (!status.enabled || !status.listening) return null
    return { url: status.url, token: this.state.token }
  }

  queue(action) {
    const pending = this.transition.then(action)
    this.transition = pending.catch(() => {})
    return pending
  }

  async startIfEnabled() {
    return this.queue(async () => {
      try {
        await this.load()
      } catch {
        // MCP 是可选能力；配置损坏或不可读不能让整个 Pisper 启动失败。
        this.state = { version: STATE_VERSION, enabled: false, token: '' }
        this.loaded = Promise.resolve()
        this.error = '内置 MCP 设置读取失败，请重新开启。'
        return this.status()
      }
      if (this.closed || !this.state.enabled) return this.status()
      await this.startListener()
      return this.status()
    })
  }

  async setEnabled(enabled) {
    if (typeof enabled !== 'boolean') throw new Error('MCP enabled must be a boolean.')
    return this.queue(async () => {
      await this.load()
      if (this.closed) throw new Error('MCP host is closed.')
      if (!enabled) {
        this.state = { ...this.state, enabled: false }
        await writeJsonAtomic(this.path, this.state, { mode: 0o600 })
        await this.stopListener()
        await this.sdk?.abortMcpHostRuns(this.runs, this.getRuntime)
        this.runs.clear()
        this.error = ''
        return this.status()
      }
      const next = {
        ...this.state,
        enabled: true,
        token: this.state.token || randomBytes(32).toString('base64url'),
      }
      await writeJsonAtomic(this.path, next, { mode: 0o600 })
      this.state = next
      await this.startListener()
      return this.status()
    })
  }

  async rotateToken() {
    return this.queue(async () => {
      await this.load()
      if (this.closed) throw new Error('MCP host is closed.')
      if (!this.state.enabled) throw new Error('请先开启 Pisper MCP 服务。')
      const next = { ...this.state, token: randomBytes(32).toString('base64url') }
      await writeJsonAtomic(this.path, next, { mode: 0o600 })
      this.state = next
      // 已建立的传输可能持有待执行调用；轮换时断开并重启监听。
      if (this.server) {
        await this.stopListener()
        await this.sdk?.abortMcpHostRuns(this.runs, this.getRuntime)
        this.runs.clear()
        if (this.state.enabled) await this.startListener()
      }
      return this.getCredentials()
    })
  }

  async startListener() {
    if (this.server) return
    // 默认关闭时不加载 MCP SDK，避免普通启动承受额外解析与内存开销。
    try {
      this.sdk = await loadSdk()
    } catch {
      this.error = '内置 MCP 组件加载失败。'
      return
    }
    const server = createServer((req, res) => {
      res.on('error', () => {})
      void this.handleRequest(req, res).catch(() =>
        sendError(res, 500, 'mcp_request_failed', 'Pisper MCP request failed.'),
      )
    })
    try {
      await new Promise((resolveListen, rejectListen) => {
        const fail = (error) => rejectListen(error)
        server.once('error', fail)
        server.listen(this.port, this.host, () => {
          server.off('error', fail)
          resolveListen()
        })
      })
      this.server = server
      this.error = ''
    } catch (error) {
      this.error =
        error?.code === 'EADDRINUSE' ? '本机 MCP 端口已被占用。' : '本机 MCP 监听启动失败。'
      try {
        server.close()
      } catch {
        // listen 失败的 server 可能从未进入可关闭状态。
      }
      // 用户开关仍保持开启；下次启动或点击开关可重试。
    }
  }

  async handleRequest(req, res) {
    const status = this.status()
    if (!status.enabled || !status.listening) {
      sendError(res, 503, 'mcp_disabled', 'Pisper MCP is disabled.')
      return
    }
    const address = req.socket.remoteAddress
    if (address !== '127.0.0.1' && address !== '::ffff:127.0.0.1') {
      sendError(res, 403, 'mcp_local_only', 'Pisper MCP accepts local connections only.')
      return
    }
    const expectedHost = `${this.host}:${status.port}`
    if (String(req.headers.host || '').toLowerCase() !== expectedHost) {
      sendError(res, 403, 'mcp_bad_host', 'Pisper MCP host is invalid.')
      return
    }
    const origin = String(req.headers.origin || '')
    if (origin && origin !== `http://${expectedHost}`) {
      sendError(res, 403, 'mcp_bad_origin', 'Pisper MCP origin is invalid.')
      return
    }
    if (req.url !== '/mcp') {
      sendError(res, 404, 'mcp_not_found', 'MCP endpoint not found.')
      return
    }
    if (!secureEqual(bearer(req.headers.authorization), this.state.token)) {
      sendError(res, 401, 'mcp_auth_required', 'Pisper MCP bearer token is required.')
      return
    }
    if (!['POST', 'GET', 'DELETE'].includes(req.method || '')) {
      sendError(res, 405, 'mcp_method_not_allowed', 'MCP method is not allowed.')
      return
    }
    if (this.activeRequests >= MAX_CONCURRENT_REQUESTS) {
      sendError(res, 429, 'mcp_busy', 'Too many concurrent MCP requests.')
      return
    }
    this.activeRequests += 1
    let mcpServer
    let transport
    try {
      const parsedBody = req.method === 'POST' ? await readBody(req) : undefined
      mcpServer = new this.sdk.McpServer({ name: 'pisper', version: '1.0.0' })
      this.sdk.registerMcpHostTools(mcpServer, {
        getRuntime: this.getRuntime,
        isEnabled: () => this.state.enabled && !this.closed,
        runs: this.runs,
      })
      transport = new this.sdk.StreamableHTTPServerTransport({
        sessionIdGenerator: undefined,
        enableJsonResponse: true,
        allowedHosts: [expectedHost],
        allowedOrigins: [`http://${expectedHost}`],
        enableDnsRebindingProtection: true,
      })
      this.transports.add(transport)
      await mcpServer.connect(transport)
      await transport.handleRequest(req, res, parsedBody)
    } catch (error) {
      const code = error?.status === 413 ? 'mcp_body_too_large' : 'mcp_invalid_request'
      sendError(
        res,
        error?.status || 400,
        code,
        error?.status === 413 ? 'MCP request body is too large.' : 'Invalid MCP request.',
      )
    } finally {
      if (transport) this.transports.delete(transport)
      await mcpServer?.close().catch(() => {})
      this.activeRequests -= 1
    }
  }

  async stopListener() {
    const server = this.server
    this.server = null
    if (!server) return
    server.closeAllConnections()
    await Promise.allSettled([...this.transports].map((transport) => transport.close()))
    this.transports.clear()
    await new Promise((resolveClose) => server.close(() => resolveClose()))
  }

  async close() {
    return this.queue(async () => {
      if (this.closed) return
      this.closed = true
      await this.stopListener()
      await this.sdk?.abortMcpHostRuns(this.runs, this.getRuntime)
      this.runs.clear()
    })
  }
}
