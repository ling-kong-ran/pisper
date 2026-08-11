import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { createStaticHandler } from './http/static-handler.mjs'
import { SponsorContentService } from './services/sponsor-content-service.mjs'
import { resolveGitCommit, UpdateCheckService } from './services/update-check-service.mjs'
import { WebDesktopPetService } from './services/web-desktop-pet-service.mjs'
import { authorizeDesktopRequest } from './desktop-sidecar-auth.mjs'

function notifyStartup(observer, stage) {
  try {
    observer?.(stage)
  } catch {
    // Startup diagnostics must never make the runtime unavailable.
  }
}

function serviceUnavailable(res) {
  const body = `${JSON.stringify({ error: 'Pisper runtime initialization failed.' })}\n`
  res.writeHead(503, {
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
  process.env.PI_CODING_AGENT_DIR = agentDir

  const desktopPet = new WebDesktopPetService({ dataDir: agentDir })
  const serveProduction = createStaticHandler(appRoot, { distRoot: frontendRoot })
  let runtime = null
  let handleApi = null
  let vite = null
  let startInitialization

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
      legacyDefaultCwds: production && basename(appRoot) === 'sidecar-runtime' ? [appRoot] : [],
      browserAutomationDriver,
      eventObserver: (payload) => {
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

  const initialized = new Promise((resolveInitialized, rejectInitialized) => {
    startInitialization = () => initialize().then(resolveInitialized, rejectInitialized)
  })

  const server = createServer(async (req, res) => {
    const address = server.address()
    const activePort = typeof address === 'object' && address ? address.port : port
    const origin = `http://${host}:${activePort}`
    const url = new URL(req.url || '/', req.headers.host ? `http://${req.headers.host}` : origin)
    if (authorizeDesktopRequest(req, res, url, { token: desktopAuthToken, origin })) return
    if (url.pathname.startsWith('/api/')) {
      try {
        await initialized
      } catch {
        serviceUnavailable(res)
        return
      }
      if (await handleApi(req, res, url)) return
    }
    if (vite) vite.middlewares(req, res)
    else await serveProduction(req, res, url)
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
      if (closing) return closing
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
