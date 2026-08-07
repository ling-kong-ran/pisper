import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { basename, join, resolve } from 'node:path'
import { createApiHandler } from './http/api-handler.mjs'
import { createStaticHandler } from './http/static-handler.mjs'
import { AgentRuntimeService } from './runtime/agent-runtime.mjs'
import { SponsorContentService } from './services/sponsor-content-service.mjs'
import { resolveGitCommit, UpdateCheckService } from './services/update-check-service.mjs'
import { WebDesktopPetService } from './services/web-desktop-pet-service.mjs'
import { authorizeDesktopRequest } from './desktop-sidecar-auth.mjs'

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
} = {}) {
  const appRoot = resolve(root || process.cwd())
  const cwd = resolve(runtimeCwd || homedir())
  const agentDir = resolve(dataDir)
  process.env.PI_CODING_AGENT_DIR = agentDir
  const packageJson = JSON.parse(await readFile(join(appRoot, 'package.json'), 'utf8'))

  const desktopPet = new WebDesktopPetService({ dataDir: agentDir })
  const runtime = new AgentRuntimeService({
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
  await runtime.init()
  const engineVersion = await readFile(
    join(appRoot, 'node_modules', '@earendil-works', 'pi-coding-agent', 'package.json'),
    'utf8',
  )
    .then((text) => JSON.parse(text).version || 'unknown')
    .catch(() => 'unknown')
  const currentCommit = await resolveGitCommit(appRoot)
  const updates = new UpdateCheckService({ currentVersion: packageJson.version, currentCommit })
  const sponsors = new SponsorContentService({
    dataDir: agentDir,
    fallbackPath: join(appRoot, 'docs', 'sponsors.json'),
    appVersion: packageJson.version,
  })
  await sponsors.init()

  let vite = null
  if (!production) {
    const { createServer: createViteServer } = await import('vite')
    vite = await createViteServer({
      root: appRoot,
      server: { middlewareMode: true, hmr: { port: Number(port) + 1 } },
      appType: 'spa',
    })
  }
  const handleApi = createApiHandler(runtime, {
    updates,
    sponsors,
    desktopPet,
    engineVersion,
  })
  const serveProduction = createStaticHandler(appRoot)
  const server = createServer(async (req, res) => {
    const address = server.address()
    const activePort = typeof address === 'object' && address ? address.port : port
    const origin = `http://${host}:${activePort}`
    const url = new URL(req.url || '/', req.headers.host ? `http://${req.headers.host}` : origin)
    if (authorizeDesktopRequest(req, res, url, { token: desktopAuthToken, origin })) return
    if (await handleApi(req, res, url)) return
    if (vite) vite.middlewares(req, res)
    else await serveProduction(req, res, url)
  })

  await new Promise((resolveListen, rejectListen) => {
    const fail = (error) => rejectListen(error)
    server.once('error', fail)
    server.listen(port, host, () => {
      server.off('error', fail)
      resolveListen()
    })
  })
  const address = server.address()
  const activePort = typeof address === 'object' && address ? address.port : port
  let closing = null
  return {
    host,
    port: activePort,
    url: `http://${host}:${activePort}`,
    dataDir: agentDir,
    runtime,
    desktopPetRunning: desktopPet.status().running,
    async close() {
      if (closing) return closing
      closing = (async () => {
        desktopPet.dispose()
        await runtime.dispose()
        await vite?.close()
        await new Promise((resolveClose) => server.close(() => resolveClose()))
      })()
      return closing
    },
  }
}
