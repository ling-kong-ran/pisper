// 运行时的组装层（工厂函数）：把 Agent 运行时、HTTP API、Vite/静态资源、更新检查、
// 赞助内容、桌面宠物等组件组装成一个可启动/可关闭的 Pisper 实例。
// 开发 Web（index.mjs）与桌面 sidecar（sidecar.mjs）都通过它构建实例。
import { createServer } from 'node:http'
import { createServer as createHttpsServer } from 'node:https'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { createStaticHandler } from './http/static-handler.mjs'
import { SponsorContentService } from './services/sponsor-content-service.mjs'
import { resolveGitCommit, UpdateCheckService } from './services/update-check-service.mjs'
import { WebDesktopPetService } from './services/web-desktop-pet-service.mjs'
import { RemoteAccessService } from './services/remote-access-service.mjs'
import { MdnsAdvertiser } from './services/mdns-advertiser.mjs'
import { authorizeDesktopRequest } from './desktop-sidecar-auth.mjs'
import { authorizeRemoteRequest } from './remote-auth.mjs'
import { ensureRemoteCertificate } from './remote-tls.mjs'
import { collectRemoteEndpoints, remoteDeviceName } from './remote-endpoints.mjs'
import { readIrohTunnelStatus } from './iroh-endpoint.mjs'
import { resolveRuntimeCapabilities } from './runtime-capabilities.mjs'

// 启动诊断回调必须无副作用：即使观察者抛错也不能影响运行时可用性。
function notifyStartup(observer, stage) {
  try {
    observer?.(stage)
  } catch {
    // Startup diagnostics must never make the runtime unavailable.
  }
}

// 运行时尚未初始化完成时的 503 响应：避免把半初始化状态当成正常服务暴露。
function serviceUnavailable(res) {
  const body = `${JSON.stringify({ error: 'Pisper runtime initialization failed.' })}\n`
  res.writeHead(503, {
    'Cache-Control': 'no-store',
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  })
  res.end(body)
}

// 请求处理中途失败时的兜底：已发送响应头则直接断开，否则回写 500，
// 保证 Node HTTP 不会因 Promise 拒绝而抛出未捕获异常。
function recoverRequestFailure(res) {
  if (res.destroyed || res.writableEnded) return
  if (res.headersSent) {
    res.destroy()
    return
  }
  const body = `${JSON.stringify({ error: 'Pisper request failed.' })}\n`
  res.writeHead(500, {
    'Cache-Control': 'no-store',
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  })
  res.end(body)
}

export async function createPisperRuntime({
  root,
  runtimeCwd = homedir(),
  dataDir = join(homedir(), '.pisper', 'agent'),
  production = false,
  port = 5173,
  host = '127.0.0.1',
  browserAutomationDriver = null,
  runtimeEventObserver = null,
  desktopAuthToken = '',
  frontendRoot = null,
  deferRuntimeInitialization = false,
  startupObserver = null,
  runtimeCapabilities = null,
  // 远程访问（移动端互联）：enabled 为 null 时跟随持久化状态，显式 true/false 覆盖之。
  remote = {},
  runtimeModuleLoader = () => import('./runtime/agent-runtime.mjs'),
  apiHandlerModuleLoader = () => import('./http/api-handler.mjs'),
} = {}) {
  const appRoot = resolve(root || process.cwd())
  const cwd = resolve(runtimeCwd || homedir())
  const agentDir = resolve(dataDir)
  const capabilities = runtimeCapabilities || (await resolveRuntimeCapabilities())
  // Pi 引擎通过该环境变量定位自己的数据目录，必须在实例化前设置。
  process.env.PI_CODING_AGENT_DIR = agentDir

  const desktopPet = new WebDesktopPetService({ dataDir: agentDir })
  const serveProduction = createStaticHandler(appRoot, { distRoot: frontendRoot })
  let runtime = null
  let handleApi = null
  let vite = null
  let startInitialization

  // ── 远程访问（移动端互联）────────────────────────────────────────────
  // 远程监听与回环监听完全解耦：回环侧行为（桌面 Cookie/免鉴权）不变；
  // 远程侧独立 HTTPS 监听，除配对接口外一律强制设备 Bearer 令牌。
  const remoteAccess = new RemoteAccessService({ dataDir: agentDir })
  const mdns = new MdnsAdvertiser()
  const remoteHost = remote.host || '0.0.0.0'
  const remotePort = Number(remote.port || 5174)
  let remoteServer = null
  let remoteTls = null
  let remoteError = null

  const remoteControl = {
    status() {
      const address = remoteServer?.address()
      const activePort = typeof address === 'object' && address ? address.port : remotePort
      const iroh = readIrohTunnelStatus(remote.irohStatusFile)
      const endpoints = remoteServer
        ? [
            ...collectRemoteEndpoints({ port: activePort, tls: true }),
            ...(iroh.available ? [iroh.endpoint] : []),
          ]
        : []
      return {
        enabled: capabilities.features.remoteAccess && remoteAccess.isEnabled(),
        listening: Boolean(remoteServer),
        host: remoteHost,
        port: activePort,
        tls: true,
        fingerprint: remoteTls?.fingerprint || null,
        deviceName: remoteDeviceName(),
        endpoints,
        iroh: {
          available: iroh.available,
          relayConnected: iroh.relayConnected,
          nodeId: iroh.nodeId,
          error: iroh.error,
        },
        mdns: mdns.status(),
        error: remoteError,
      }
    },
    async setEnabled(enabled) {
      if (!capabilities.features.remoteAccess)
        throw new Error('当前 Runtime 不支持远程访问服务端。')
      remoteAccess.setEnabled(enabled)
      if (enabled) await startRemote()
      else await stopRemote()
      return remoteControl.status()
    },
    // 配对二维码负载：版本化 JSON，移动端扫码后凭 code 换设备令牌。
    qrPayload({ code }) {
      const status = remoteControl.status()
      return {
        v: 1,
        name: status.deviceName,
        endpoints: status.endpoints,
        fp: status.fingerprint,
        code,
      }
    },
  }

  const startRemote = async () => {
    if (remoteServer) return true
    remoteError = null
    try {
      remoteTls = await ensureRemoteCertificate({ dataDir: agentDir })
      remoteServer = createHttpsServer({ key: remoteTls.key, cert: remoteTls.cert }, (req, res) => {
        res.on('error', () => {})
        void handleRequest(req, res, { remote: true }).catch(() => recoverRequestFailure(res))
      })
      await new Promise((resolveListen, rejectListen) => {
        const fail = (error) => rejectListen(error)
        remoteServer.once('error', fail)
        remoteServer.listen(remotePort, remoteHost, () => {
          remoteServer.off('error', fail)
          resolveListen()
        })
      })
      const status = remoteControl.status()
      // mDNS 广播是软依赖：失败仅记录，二维码/手动输入路径不受影响。
      await mdns.start({
        name: status.deviceName,
        port: status.port,
        txt: { v: '1', fp: status.fingerprint || '', name: status.deviceName, tls: '1' },
      })
      return true
    } catch (error) {
      remoteError = error instanceof Error ? error.message : String(error)
      if (remoteServer) {
        const server = remoteServer
        remoteServer = null
        await new Promise((resolveClose) => server.close(() => resolveClose()))
      }
      return false
    }
  }

  const stopRemote = async () => {
    await mdns.stop()
    if (remoteServer) {
      const server = remoteServer
      remoteServer = null
      await new Promise((resolveClose) => server.close(() => resolveClose()))
    }
  }

  // 运行时初始化：并行加载包元数据与核心模块，构造 AgentRuntimeService 及外围服务。
  // 使用模块加载器注入（runtimeModuleLoader/apiHandlerModuleLoader）便于测试替换。
  const initialize = async () => {
    notifyStartup(startupObserver, 'runtime-modules-loading')
    const [packageText, runtimeModule, apiHandlerModule, engineVersion, currentCommit] =
      await Promise.all([
        readFile(join(appRoot, 'package.json'), 'utf8'),
        runtimeModuleLoader(),
        apiHandlerModuleLoader(),
        readFile(
          join(appRoot, 'node_modules', '@earendil-works', 'pi-coding-agent', 'package.json'),
          'utf8',
        )
          .then((text) => JSON.parse(text).version || 'unknown')
          .catch(() => 'unknown'),
        resolveGitCommit(appRoot),
      ])
    notifyStartup(startupObserver, 'runtime-modules-loaded')

    const packageJson = JSON.parse(packageText)
    const { AgentRuntimeService } = runtimeModule
    runtime = new AgentRuntimeService({
      cwd,
      dataDir: agentDir,
      appVersion: packageJson.version,
      // 桌面 SEA 包把运行时装在 sidecar-runtime 下，旧版会话可能把该目录记录为 cwd，
      // 作为 legacy 默认工作目录保留，避免历史会话恢复时找不到路径。
      legacyDefaultCwds: production && basename(appRoot) === 'sidecar-runtime' ? [appRoot] : [],
      browserAutomationDriver,
      capabilities,
      eventObserver: (payload) => {
        // 事件观察全部 best-effort：桌面宠物或外部观察者抛错不能中断 Agent 流。
        try {
          desktopPet.observeRuntimeEvent(payload)
        } catch {
          // Browser pet updates are best-effort and must not interrupt Agent streams.
        }
        try {
          runtimeEventObserver?.(payload)
        } catch {
          // External desktop observers remain best-effort.
        }
      },
    })
    notifyStartup(startupObserver, 'runtime-created')
    await runtime.init({
      startupObserver: (stage) => notifyStartup(startupObserver, `runtime-${stage}`),
    })

    // 更新检查与赞助内容都依赖 app 版本/提交信息，放在运行时初始化之后构建。
    const updates = new UpdateCheckService({
      currentVersion: packageJson.version,
      currentCommit,
    })
    const sponsors = new SponsorContentService({
      dataDir: agentDir,
      fallbackPath: join(appRoot, 'docs', 'sponsors.json'),
      appVersion: packageJson.version,
    })
    await sponsors.init()
    // 仅开发模式注入 Vite 中间件；生产环境直接托管 dist 静态资源。
    if (!production) {
      const { createServer: createViteServer } = await import('vite')
      vite = await createViteServer({
        root: appRoot,
        server: { middlewareMode: true, hmr: { port: Number(port) + 1 } },
        appType: 'spa',
      })
    }
    handleApi = apiHandlerModule.createApiHandler(runtime, {
      updates,
      sponsors,
      desktopPet,
      engineVersion,
      remoteAccess,
      remoteControl,
    })
    notifyStartup(startupObserver, 'runtime-initialized')
    return runtime
  }

  // initialized 把异步初始化包装成 Promise：请求可在初始化完成前到达（await 排队），
  // 而 deferRuntimeInitialization 模式下 HTTP 先监听、初始化延后触发。
  const initialized = new Promise((resolveInitialized, rejectInitialized) => {
    startInitialization = () => initialize().then(resolveInitialized, rejectInitialized)
  })

  const handleRequest = async (req, res, { remote: isRemoteListener = false } = {}) => {
    const address = server.address()
    const activePort = typeof address === 'object' && address ? address.port : port
    const origin = `http://${host}:${activePort}`
    const url = new URL(req.url || '/', req.headers.host ? `http://${req.headers.host}` : origin)
    // 鉴权分层：回环监听走桌面 Cookie 引导；远程监听走设备 Bearer 令牌。
    if (isRemoteListener) {
      if (authorizeRemoteRequest(req, res, url, { remoteAccess })) return
    } else if (authorizeDesktopRequest(req, res, url, { token: desktopAuthToken, origin })) {
      return
    }
    if (url.pathname.startsWith('/api/')) {
      // API 请求必须等运行时就绪；初始化失败统一返回 503。
      try {
        await initialized
      } catch {
        serviceUnavailable(res)
        return
      }
      // handleApi 返回 true 表示已消费该请求（命中路由），false 则继续走静态资源。
      if (await handleApi(req, res, url)) return
    }
    if (vite) vite.middlewares(req, res)
    else await serveProduction(req, res, url)
  }

  const server = createServer((req, res) => {
    // EventEmitter does not observe a Promise returned by an async request listener.
    // Keep a rejected route or a disconnected response from becoming an uncaught process error.
    res.on('error', () => {})
    void handleRequest(req, res).catch(() => recoverRequestFailure(res))
  })

  if (!deferRuntimeInitialization) {
    startInitialization()
    await initialized
  }
  await new Promise((resolveListen, rejectListen) => {
    const fail = (error) => rejectListen(error)
    server.once('error', fail)
    server.listen(port, host, () => {
      server.off('error', fail)
      resolveListen()
    })
  })
  notifyStartup(startupObserver, 'http-listening')
  // 延迟模式下先返回实例（端口已监听），初始化放到下一个事件循环轮次执行。
  if (deferRuntimeInitialization) setImmediate(startInitialization)

  // 远程监听：显式开关优先，其次持久化状态。启动失败只记录状态，不影响主服务。
  const remoteRequested =
    capabilities.features.remoteAccess && (remote.enabled ?? remoteAccess.isEnabled())
  if (capabilities.features.remoteAccess && remote.enabled === true) remoteAccess.setEnabled(true)
  if (remoteRequested) await startRemote()

  const address = server.address()
  const activePort = typeof address === 'object' && address ? address.port : port
  let closing = null
  return {
    host,
    port: activePort,
    url: `http://${host}:${activePort}`,
    dataDir: agentDir,
    capabilities,
    remoteControl,
    get runtime() {
      return runtime
    },
    initialized,
    desktopPetRunning: desktopPet.status().running,
    async close() {
      // closing 缓存 Promise，保证多次 close 只执行一次完整清理。
      closing = (async () => {
        desktopPet.dispose()
        await stopRemote()
        await initialized.catch(() => null)
        await runtime?.dispose()
        await vite?.close()
        await new Promise((resolveClose) => server.close(() => resolveClose()))
      })()
      return closing
    },
  }
}
