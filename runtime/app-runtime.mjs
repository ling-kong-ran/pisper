// 运行时的组装层（工厂函数）：把 Agent 运行时、HTTP API、Vite/静态资源、更新检查、
// 赞助内容、桌面宠物等组件组装成一个可启动/可关闭的 Pisper 实例。
// 开发 Web（index.mjs）与桌面 sidecar（sidecar.mjs）都通过它构建实例。
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { createStaticHandler } from './http/static-handler.mjs'
import { SponsorContentService } from './services/sponsor-content-service.mjs'
import { resolveGitCommit, UpdateCheckService } from './services/update-check-service.mjs'
import { WebDesktopPetService } from './services/web-desktop-pet-service.mjs'
import { authorizeDesktopRequest } from './desktop-sidecar-auth.mjs'

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
  runtimeModuleLoader = () => import('./runtime/agent-runtime.mjs'),
  apiHandlerModuleLoader = () => import('./http/api-handler.mjs'),
} = {}) {
  const appRoot = resolve(root || process.cwd())
  const cwd = resolve(runtimeCwd || homedir())
  const agentDir = resolve(dataDir)
  // Pi 引擎通过该环境变量定位自己的数据目录，必须在实例化前设置。
  process.env.PI_CODING_AGENT_DIR = agentDir

  const desktopPet = new WebDesktopPetService({ dataDir: agentDir })
  const serveProduction = createStaticHandler(appRoot, { distRoot: frontendRoot })
  let runtime = null
  let handleApi = null
  let vite = null
  let startInitialization

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
    })
    notifyStartup(startupObserver, 'runtime-initialized')
    return runtime
  }

  // initialized 把异步初始化包装成 Promise：请求可在初始化完成前到达（await 排队），
  // 而 deferRuntimeInitialization 模式下 HTTP 先监听、初始化延后触发。
  const initialized = new Promise((resolveInitialized, rejectInitialized) => {
    startInitialization = () => initialize().then(resolveInitialized, rejectInitialized)
  })

  const handleRequest = async (req, res) => {
    const address = server.address()
    const activePort = typeof address === 'object' && address ? address.port : port
    const origin = `http://${host}:${activePort}`
    const url = new URL(req.url || '/', req.headers.host ? `http://${req.headers.host}` : origin)
    // 先做桌面鉴权：桌面引导路径或缺失 Cookie 的请求在此被拦截，其余请求放行。
    if (authorizeDesktopRequest(req, res, url, { token: desktopAuthToken, origin })) return
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

  const address = server.address()
  const activePort = typeof address === 'object' && address ? address.port : port
  let closing = null
  return {
    host,
    port: activePort,
    url: `http://${host}:${activePort}`,
    dataDir: agentDir,
    get runtime() {
      return runtime
    },
    initialized,
    desktopPetRunning: desktopPet.status().running,
    async close() {
      // closing 缓存 Promise，保证多次 close 只执行一次完整清理。
      closing = (async () => {
        desktopPet.dispose()
        await initialized.catch(() => null)
        await runtime?.dispose()
        await vite?.close()
        await new Promise((resolveClose) => server.close(() => resolveClose()))
      })()
      return closing
    },
  }
}
