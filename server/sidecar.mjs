import { randomBytes } from 'node:crypto'
import { dirname, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { createPisperServer } from './app-server.mjs'
import { resolveAgentDataDir } from './data-dir-migration.mjs'

const serverDir = dirname(fileURLToPath(import.meta.url))
const defaultRoot = resolve(serverDir, '..')
const root = resolve(process.env.PISPER_APP_ROOT || defaultRoot)
const host = '127.0.0.1'
const token = String(process.env.PISPER_DESKTOP_TOKEN || randomBytes(32).toString('base64url'))
const parentPid = Number(process.env.PISPER_PARENT_PID || 0)

process.env.PI_SKIP_VERSION_CHECK ||= '1'
process.env.PI_TELEMETRY ||= '0'

const pisper = await createPisperServer({
  root,
  runtimeCwd: process.env.PISPER_WORKSPACE_DIR || undefined,
  dataDir: resolveAgentDataDir(),
  production: true,
  port: 0,
  host,
  desktopAuthToken: token,
})

const bootstrapUrl = `${pisper.url}/_pisper/desktop/bootstrap?token=${encodeURIComponent(token)}`
process.stdout.write(
  `PISPER_SIDECAR_READY ${JSON.stringify({
    url: pisper.url,
    bootstrapUrl,
    pid: process.pid,
    desktopPetRunning: pisper.desktopPetRunning,
  })}\n`,
)

let shuttingDown = false
async function shutdown(code = 0) {
  if (shuttingDown) return
  shuttingDown = true
  try {
    await pisper.close()
  } finally {
    process.exit(code)
  }
}

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
