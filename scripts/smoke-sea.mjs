import { spawn } from 'node:child_process'
import { mkdtemp, rm } from 'node:fs/promises'
import { homedir, tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const executable = join(
  root,
  'release',
  'sea',
  process.platform === 'win32' ? 'pisper-sidecar.exe' : 'pisper-sidecar',
)
const runtimeRoot = join(root, 'release', 'sea', 'runtime')
const prefix = 'PISPER_SIDECAR_READY '
const token = 'pisper-sea-smoke-token'
const dataDir = await mkdtemp(join(tmpdir(), 'pisper-sea-smoke-'))
const child = spawn(executable, [], {
  cwd: root,
  env: {
    ...process.env,
    PISPER_AGENT_DIR: dataDir,
    PISPER_APP_ROOT: runtimeRoot,
    PISPER_DESKTOP_TOKEN: token,
    PISPER_EXIT_ON_STDIN_CLOSE: '1',
  },
  stdio: ['pipe', 'pipe', 'pipe'],
})
let stderr = ''
child.stderr.on('data', (chunk) => {
  stderr += String(chunk)
})

function readyPayload() {
  return new Promise((resolveReady, rejectReady) => {
    const timeout = setTimeout(
      () => rejectReady(new Error(`SEA readiness timed out.\n${stderr}`)),
      30_000,
    )
    let buffered = ''
    child.stdout.on('data', (chunk) => {
      buffered += String(chunk)
      const lines = buffered.split(/\r?\n/)
      buffered = lines.pop() || ''
      for (const line of lines) {
        if (!line.startsWith(prefix)) continue
        clearTimeout(timeout)
        resolveReady(JSON.parse(line.slice(prefix.length)))
      }
    })
    child.once('exit', (code) => {
      clearTimeout(timeout)
      rejectReady(new Error(`SEA exited before readiness (${code}).\n${stderr}`))
    })
  })
}

function waitForExit() {
  return new Promise((resolveExit, rejectExit) => {
    if (child.exitCode !== null) {
      resolveExit(child.exitCode)
      return
    }
    const timeout = setTimeout(() => rejectExit(new Error('SEA shutdown timed out.')), 15_000)
    child.once('exit', (code) => {
      clearTimeout(timeout)
      resolveExit(code)
    })
  })
}

async function api(url, cookie, path, init = {}) {
  return fetch(`${url}${path}`, {
    ...init,
    headers: {
      Cookie: cookie,
      Origin: url,
      ...(init.body ? { 'Content-Type': 'application/json' } : {}),
      ...init.headers,
    },
  })
}

try {
  const ready = await readyPayload()
  const unauthorized = await fetch(`${ready.url}/api/config`)
  if (unauthorized.status !== 401) throw new Error(`Expected unauthenticated 401, received ${unauthorized.status}.`)

  const bootstrap = await fetch(ready.bootstrapUrl, { redirect: 'manual' })
  if (bootstrap.status !== 302) throw new Error(`Expected bootstrap 302, received ${bootstrap.status}.`)
  const cookie = `__pisper_desktop=${encodeURIComponent(token)}`

  const config = await api(ready.url, cookie, '/api/config')
  if (!config.ok) throw new Error(`Config API failed with ${config.status}.`)

  const created = await api(ready.url, cookie, '/api/sessions', {
    method: 'POST',
    body: JSON.stringify({ name: 'SEA smoke test' }),
  })
  if (created.status !== 201) throw new Error(`Session creation failed with ${created.status}.`)
  const session = await created.json()
  if (resolve(session.cwd) !== resolve(homedir())) {
    throw new Error(`Expected default workspace ${homedir()}, received ${session.cwd}.`)
  }

  const prompt = await api(ready.url, cookie, '/api/chat', {
    method: 'POST',
    body: JSON.stringify({ sessionId: session.id, message: 'SEA runtime smoke test' }),
  })
  const events = await prompt.text()
  if (!prompt.ok || !events.trim()) throw new Error(`Agent activation failed with ${prompt.status}.`)

  child.stdin.end('shutdown\n')
  const exitCode = await waitForExit()
  if (exitCode !== 0) throw new Error(`SEA exited with code ${exitCode}.\n${stderr}`)
  console.log(`SEA smoke passed: ${ready.url}, agent runtime activated, exit ${exitCode}.`)
} finally {
  if (child.exitCode === null) child.kill()
  await rm(dataDir, { recursive: true, force: true })
}
