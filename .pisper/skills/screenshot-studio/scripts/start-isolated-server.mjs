// Start, reset, or stop an isolated Pisper instance for screenshot capture.
// All paths and network settings come from screenshot-config.mjs and support env overrides.
import { spawn } from 'node:child_process'
import {
  createWriteStream,
  mkdirSync,
  readFileSync,
  rmSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs'
import { resolve } from 'node:path'
import { AGENT_DIR, BASE_URL, PORT, ROOT, RUN_DIR, WORKSPACE_DIR } from './screenshot-config.mjs'

const PID_FILE = resolve(RUN_DIR, 'server.pid')
const RESET = process.argv.includes('--reset')
const STOP = process.argv.includes('--stop')

async function healthReady(timeoutMs = 40_000) {
  const started = Date.now()
  while (Date.now() - started < timeoutMs) {
    try {
      const res = await fetch(`${BASE_URL}/api/health`)
      if (res.ok) return true
    } catch {
      /* not up yet */
    }
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
  return false
}

async function stopExisting() {
  let pid = 0
  try {
    pid = Number(readFileSync(PID_FILE, 'utf8').trim())
  } catch {
    /* no pid file */
  }
  if (Number.isInteger(pid) && pid > 0) {
    try {
      process.kill(pid, 'SIGTERM')
    } catch {
      /* already gone */
    }
    const deadline = Date.now() + 5_000
    while (Date.now() < deadline) {
      try {
        process.kill(pid, 0)
        await new Promise((resolve) => setTimeout(resolve, 100))
      } catch {
        break
      }
    }
  }
  try {
    unlinkSync(PID_FILE)
  } catch {
    /* no pid file */
  }
}

async function main() {
  await stopExisting()
  if (STOP) {
    console.log('Isolated Pisper stopped.')
    return
  }
  if (RESET) {
    rmSync(AGENT_DIR, { recursive: true, force: true })
    rmSync(RUN_DIR, { recursive: true, force: true })
  }
  mkdirSync(RUN_DIR, { recursive: true })
  mkdirSync(AGENT_DIR, { recursive: true })
  const logStream = createWriteStream(resolve(RUN_DIR, 'server.log'))
  const child = spawn(
    process.execPath,
    [resolve(ROOT, 'runtime/index.mjs')],
    {
      cwd: ROOT,
      env: {
        ...process.env,
        PISPER_AGENT_DIR: AGENT_DIR,
        PISPER_WORKSPACE_DIR: WORKSPACE_DIR,
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
    console.error(`Server did not become healthy at ${BASE_URL}. Check ${RUN_DIR}.`)
    process.exit(1)
  }
  console.log(`Isolated Pisper ready at ${BASE_URL} (pid ${child.pid})`)
  console.log(`Agent data dir: ${AGENT_DIR}`)
  // The child's stdio pipes keep this parent's event loop alive; exit explicitly.
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
