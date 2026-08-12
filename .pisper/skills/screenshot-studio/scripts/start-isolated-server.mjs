// Start (or restart) an isolated Pisper dev instance for screenshot capture.
// Uses port 5180 and a fresh agent data dir under generated/screenshot-agent/.
// Never touches the user's ~/.pisper/agent or the port-5173 dev server.
import { spawn } from 'node:child_process'
import { createWriteStream, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const ROOT = resolve(import.meta.dirname, '../../../..')
const PORT = Number(process.env.SCREENSHOT_PORT || 5180)
const AGENT_DIR = resolve(ROOT, 'generated/screenshot-agent')
const RUN_DIR = resolve(ROOT, 'generated/screenshot-run')
const PID_FILE = resolve(RUN_DIR, 'server.pid')

async function healthReady(timeoutMs = 40_000) {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    try {
      const res = await fetch(`http://127.0.0.1:${PORT}/api/health`)
      if (res.ok) return true
    } catch {
      /* not up yet */
    }
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
  return false
}

async function stopExisting() {
  let pid = ''
  try {
    pid = readFileSync(PID_FILE, 'utf8').trim()
  } catch {
    /* no pid file */
  }
  if (pid) {
    try {
      process.kill(Number(pid), 'SIGTERM')
    } catch {
      /* already gone */
    }
    await new Promise((resolve) => setTimeout(resolve, 1000))
  }
}

async function main() {
  mkdirSync(RUN_DIR, { recursive: true })
  mkdirSync(AGENT_DIR, { recursive: true })
  await stopExisting()
  const logStream = createWriteStream(resolve(RUN_DIR, 'server.log'))
  const child = spawn(
    process.execPath,
    [resolve(ROOT, 'runtime/index.mjs')],
    {
      cwd: ROOT,
      env: {
        ...process.env,
        PISPER_AGENT_DIR: AGENT_DIR,
        PISPER_WORKSPACE_DIR: ROOT,
        PORT: String(PORT),
        PISPER_OPEN_BROWSER: '0',
      },
      stdio: ['ignore', 'pipe', 'pipe'],
      detached: true,
    },
  )
  child.stdout.pipe(logStream)
  child.stderr.pipe(logStream)
  child.on('error', (error) => console.error('spawn error:', error.message))
  child.on('exit', (code, signal) => logStream.end(`child exited code=${code} signal=${signal}\n`))
  child.unref()
  writeFileSync(PID_FILE, String(child.pid))
  if (!(await healthReady())) {
    console.error(`Server did not become healthy on port ${PORT}. Check generated/screenshot-run/`)
    process.exit(1)
  }
  console.log(`Isolated Pisper ready at http://127.0.0.1:${PORT} (pid ${child.pid})`)
  console.log(`Agent data dir: ${AGENT_DIR}`)
  // The child's stdio pipes keep this parent's event loop alive; exit explicitly.
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
