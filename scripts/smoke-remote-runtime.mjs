// 使用隔离 PC Runtime、独立移动客户端和本机模型夹具验证扫码到任务的完整链路。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createServer } from 'node:http'
import { mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const publicRelay = process.argv.includes('--public')
const paths = process.argv.slice(2).filter((argument) => !argument.startsWith('--'))
const suffix = process.platform === 'win32' ? '.exe' : ''
const runtime = resolve(paths[0] || `runtime-rs/target/debug/pisper-server${suffix}`)
const mobile = resolve(
  paths[1] || `crates/pisper-remote-transport/target/debug/examples/remote-runtime-smoke${suffix}`,
)
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-remote-smoke-'))
const agent = join(sandbox, 'agent')
const workspace = join(sandbox, 'workspace')
// 模拟原 Tauri 身份文件位置；断言 Runtime 不在 agent 下另建身份或迁移证书。
const desktop = join(sandbox, 'desktop')
const cookie = 'synthetic-remote-smoke-cookie'
const env = Object.fromEntries(
  Object.entries(process.env).filter(([key]) =>
    /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP|TMPDIR)$/i.test(key),
  ),
)
let pc,
  pcDone,
  client,
  clientDone,
  base,
  modelRequests = 0
let diagnostics = ''
const checks = []

function startProcess(command, args, options) {
  const child = spawn(command, args, { windowsHide: true, ...options })
  const done = new Promise((resolveExit, reject) => {
    child.once('error', reject)
    child.once('close', (code, signal) => resolveExit({ code, signal }))
  })
  child.stderr.on('data', (chunk) => {
    diagnostics = (diagnostics + chunk).slice(-8000)
  })
  child.stdin.on('error', () => {})
  return [child, done]
}

async function startPc() {
  base = undefined
  ;[pc, pcDone] = startProcess(runtime, [], {
    cwd: workspace,
    stdio: ['pipe', 'pipe', 'pipe'],
    env: {
      ...env,
      HOME: sandbox,
      USERPROFILE: sandbox,
      PISPER_AGENT_DIR: agent,
      PI_CODING_AGENT_DIR: agent,
      PISPER_RS_DATA_DIR: agent,
      PISPER_WORKSPACE_DIR: workspace,
      PISPER_APP_ROOT: root,
      PISPER_DESKTOP_TOKEN: cookie,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PISPER_REMOTE_PORT: '0',
      PISPER_REMOTE_RELAY_ONLY: publicRelay ? '1' : '0',
      PISPER_REMOTE_OFFLINE: publicRelay ? '0' : '1',
      PISPER_IROH_STATUS_FILE: join(desktop, 'iroh-tunnel-status.json'),
      PI_SKIP_VERSION_CHECK: '1',
      PI_TELEMETRY: '0',
      RUST_LOG: process.env.PISPER_REMOTE_SMOKE_LOG || 'pisper_server=info',
    },
  })
  let pending = ''
  pc.stdout.on('data', (chunk) => {
    pending += chunk
    const lines = pending.split('\n')
    pending = lines.pop()
    for (const line of lines) {
      if (line.startsWith('PISPER_SIDECAR_READY ')) base = JSON.parse(line.slice(21)).url
      else diagnostics = (diagnostics + line + '\n').slice(-16000)
    }
  })
  const deadline = Date.now() + 60000
  while (!base && pc.exitCode === null && Date.now() < deadline) await delay(50)
  assert.ok(base, 'Isolated PC Runtime did not start')
}

async function stopPc() {
  if (!pc) return
  const current = pc
  let forced = false
  const timer = setTimeout(() => {
    forced = true
    current.kill('SIGKILL')
  }, 20000)
  try {
    if (current.exitCode === null && current.signalCode === null) current.stdin.end('shutdown\n')
    assert.deepEqual(await pcDone, { code: 0, signal: null })
    assert.equal(forced, false, 'Runtime did not stop its connections')
  } finally {
    clearTimeout(timer)
    pc = undefined
  }
}

const model = createServer(async (request, response) => {
  try {
    let raw = ''
    for await (const chunk of request) {
      raw += chunk
      assert.ok(raw.length < 4 * 1024 * 1024)
    }
    const body = JSON.parse(raw)
    modelRequests++
    response.writeHead(200, { 'Content-Type': 'text/event-stream' })
    for (const delta of [{ role: 'assistant', content: 'P2P_中文_OK' }, {}]) {
      response.write(
        `data: ${JSON.stringify({ id: 'fixture', object: 'chat.completion.chunk', created: 1, model: body.model, choices: [{ index: 0, delta, finish_reason: delta.content ? null : 'stop' }] })}\n\n`,
      )
    }
    response.end('data: [DONE]\n\n')
  } catch {
    response.writeHead(500)
    response.end('fixture failed')
  }
})

let restartWork = Promise.resolve()
try {
  await Promise.all([agent, workspace, desktop].map((path) => mkdir(path)))
  await new Promise((resolveListen) => model.listen(0, '127.0.0.1', resolveListen))
  const provider = 'remote-fixture'
  for (const [name, value] of Object.entries({
    'models.json': {
      providers: {
        [provider]: {
          api: 'openai-completions',
          baseUrl: `http://127.0.0.1:${model.address().port}/v1`,
          models: [
            {
              id: provider,
              name: 'Remote fixture',
              reasoning: false,
              input: ['text'],
              contextWindow: 128000,
              maxTokens: 1000,
            },
          ],
        },
      },
    },
    'auth.json': { [provider]: { type: 'api_key', key: 'synthetic-remote-fixture-key' } },
    'settings.json': { defaultProvider: provider, defaultModel: provider },
    'pisper.json': {},
  }))
    await writeFile(join(agent, name), JSON.stringify(value))
  await startPc()
  ;[client, clientDone] = startProcess(mobile, [], {
    cwd: workspace,
    env,
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  const sendConfig = () =>
    client.stdin.write(JSON.stringify({ base, cookie, public: publicRelay }) + '\n')
  sendConfig()
  let pending = '',
    passed = false
  client.stdout.on('data', (chunk) => {
    pending += chunk
    const lines = pending.split('\n')
    pending = lines.pop()
    for (const line of lines.filter(Boolean)) {
      const message = JSON.parse(line)
      if (message.check) {
        checks.push(message.check)
        console.log(JSON.stringify(message))
      }
      if (message.status === 'passed') passed = true
      if (message.phase === 'restart') {
        restartWork = (async () => {
          const cert = await readFile(join(agent, 'remote-tls.json'))
          const key = await readFile(join(desktop, 'iroh-secret.key'))
          const devices = JSON.parse(await readFile(join(agent, 'remote-access.json'), 'utf8'))
          assert.ok(devices.devices.length > 0)
          assert.ok(devices.devices.every((device) => device.tokenHash && !device.token))
          await assert.rejects(readFile(join(agent, 'iroh-secret.key')), { code: 'ENOENT' })
          await stopPc()
          await startPc()
          assert.deepEqual(await readFile(join(agent, 'remote-tls.json')), cert)
          assert.deepEqual(await readFile(join(desktop, 'iroh-secret.key')), key)
          sendConfig()
        })()
        restartWork.catch(() => client.stdin.end())
      }
    }
  })
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    client.kill('SIGKILL')
  }, 240000)
  try {
    assert.deepEqual(await clientDone, { code: 0, signal: null })
    assert.equal(timedOut, false, 'Remote integration test timed out')
    await restartWork
    assert.equal(passed, true)
    assert.equal(modelRequests, 1)
    await stopPc()
    console.log(
      JSON.stringify(
        { status: 'passed', mode: publicRelay ? 'public-relay-only' : 'offline-direct', checks },
        null,
        2,
      ),
    )
  } finally {
    clearTimeout(timer)
  }
} catch (error) {
  console.error(
    diagnostics
      .replaceAll(sandbox, '<fixture>')
      .replaceAll(root, '<repository>')
      .replaceAll(cookie, '<cookie>'),
  )
  throw error
} finally {
  if (client && client.exitCode === null && client.signalCode === null) {
    client.kill('SIGKILL')
    await clientDone
  }
  await restartWork.catch(() => {})
  try {
    await stopPc()
  } finally {
    model.closeAllConnections()
    await new Promise((resolveClose) => model.close(resolveClose))
    assert.equal(await realpath(sandbox), sandbox, 'Refuse to remove a redirected fixture root')
    await rm(sandbox, { recursive: true })
  }
}
