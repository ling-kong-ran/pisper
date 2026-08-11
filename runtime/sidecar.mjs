import { randomBytes } from 'node:crypto'
import { dirname, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { createPisperRuntime } from './app-runtime.mjs'
import { resolveAgentDataDir } from './data-dir-migration.mjs'

const serverDir = dirname(fileURLToPath(import.meta.url))
const defaultRoot = resolve(serverDir, '..')
const root = resolve(process.env.PISPER_APP_ROOT || defaultRoot)
const host = '127.0.0.1'
const token = String(process.env.PISPER_DESKTOP_TOKEN || randomBytes(32).toString('base64url'))
const parentPid = Number(process.env.PISPER_PARENT_PID || 0)
let previousStageAt = 0
const startupTiming = process.env.PISPER_STARTUP_TIMING === '1'

function reportStartupStage(stage) {
  if (!startupTiming) return
  const elapsedMs = Math.round(performance.now())
  process.stderr.write(
    `PISPER_SIDECAR_STAGE ${JSON.stringify({
      stage,
      elapsedMs,
      deltaMs: elapsedMs - previousStageAt,
    })}\n`,
  )
  previousStageAt = elapsedMs
}

process.env.PI_SKIP_VERSION_CHECK ||= '1'
process.env.PI_TELEMETRY ||= '0'
reportStartupStage('entry')

let pisper = null
let shuttingDown = false
async function shutdown(code = 0) {
  if (shuttingDown) return
  shuttingDown = true
  try {
    await pisper?.close()
  } finally {
    process.exit(code)
  }
}

pisper = await createPisperRuntime({
  root,
  runtimeCwd: process.env.PISPER_WORKSPACE_DIR || undefined,
  dataDir: resolveAgentDataDir(),
  production: true,
  port: 0,
  host,
  desktopAuthToken: token,
  frontendRoot: process.env.PISPER_FRONTEND_ROOT || null,
  deferRuntimeInitialization: true,
  startupObserver: reportStartupStage,
})

const bootstrapUrl = `${pisper.url}/_pisper/desktop/bootstrap?token=${encodeURIComponent(token)}`
reportStartupStage('ready')
process.stdout.write(
  `PISPER_SIDECAR_READY ${JSON.stringify({
    url: pisper.url,
    bootstrapUrl,
    pid: process.pid,
    desktopPetRunning: pisper.desktopPetRunning,
    startupMs: Math.round(performance.now()),
  })}\n`,
)
void pisper.initialized.catch((error) => {
  reportStartupStage('failed')
  console.error('Pisper sidecar runtime initialization failed.', error)
  void shutdown(1)
})

const input = createInterface({ input: process.stdin, terminal: false })
input.on('line', (line) => {
  if (line.trim().toLowerCase() === 'shutdown') void shutdown(0)
})
if (process.env.PISPER_EXIT_ON_STDIN_CLOSE === '1') input.on('close', () => void shutdown(0))

if (Number.isInteger(parentPid) && parentPid > 0) {
  const parentCheck = setInterval(() => {
    try {
      process.kill(parentPid, 0)
    } catch {
      void shutdown(0)
    }
  }, 2_000)
  parentCheck.unref()
}

process.on('SIGINT', () => void shutdown(0))
process.on('SIGTERM', () => void shutdown(0))
process.on('uncaughtException', (error) => {
  console.error(error)
  void shutdown(1)
})
process.on('unhandledRejection', (error) => {
  console.error(error)
  void shutdown(1)
})
