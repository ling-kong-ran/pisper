// 用隔离的真实 Runtime 验证思考能力契约；不发送模型请求，不读取个人配置。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  appendFile,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(
  process.argv[2] ||
    join(
      root,
      'runtime-rs/target/debug',
      process.platform === 'win32' ? 'pisper-server.exe' : 'pisper-server',
    ),
)
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-thinking-contract-'))
const agent = join(sandbox, 'agent')
const workspace = join(sandbox, 'workspace')
const provider = 'thinking-contract-fixture'
const token = 'synthetic_thinking_contract_token'
const reasoningLevels = ['off', 'minimal', 'low', 'medium', 'high']
let child, ended, base

async function start() {
  let output = ''
  let failure
  child = spawn(executable, [], {
    cwd: workspace,
    env: {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([key]) =>
          /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP|TMPDIR)$/i.test(key),
        ),
      ),
      HOME: sandbox,
      USERPROFILE: sandbox,
      PISPER_AGENT_DIR: agent,
      PI_CODING_AGENT_DIR: agent,
      PISPER_RS_DATA_DIR: agent,
      PISPER_WORKSPACE_DIR: workspace,
      PISPER_APP_ROOT: root,
      PISPER_DESKTOP_TOKEN: token,
      PISPER_EXIT_ON_STDIN_CLOSE: '1',
      PI_SKIP_VERSION_CHECK: '1',
      PI_TELEMETRY: '0',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  ended = new Promise((resolveExit) => child.once('close', resolveExit))
  child.once('error', (error) => {
    failure = error
  })
  child.stderr.on('data', () => {})
  child.stdout.on('data', (chunk) => {
    output += chunk
    const line = output.split('\n').find((value) => value.startsWith('PISPER_SIDECAR_READY '))
    if (line) base = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length)).url
  })
  const deadline = Date.now() + 30000
  while (!base && !failure && child.exitCode === null && Date.now() < deadline) await delay(50)
  assert.ok(base && !failure, 'Isolated thinking contract Runtime must start')
}

async function stop() {
  if (child?.pid && child.exitCode === null) {
    child.stdin.end('shutdown\n')
    let forced = false
    const timeout = setTimeout(() => {
      forced = true
      child.kill('SIGKILL')
    }, 15000)
    try {
      const code = await ended
      assert.equal(forced, false, 'Fixture shutdown must release its own resources')
      assert.equal(code, 0, 'Fixture Runtime must exit cleanly')
    } finally {
      clearTimeout(timeout)
    }
  }
  base = undefined
}

async function json(path, method = 'GET', body, expectedStatus = 200) {
  const response = await fetch(base + path, {
    method,
    headers: {
      Origin: base,
      Cookie: `__pisper_desktop=${token}`,
      'Content-Type': 'application/json',
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
    signal: AbortSignal.timeout(20000),
  })
  assert.equal(response.status, expectedStatus, `${method} ${path}: response status`)
  return response.json()
}

function verifyState(state, id, modelId, level, levels) {
  assert.equal(state.id, id)
  assert.equal(state.model, `${provider}/${modelId}`)
  assert.equal(state.thinkingLevel, level)
  assert.deepEqual(state.availableLevels, levels)
  assert.deepEqual(state.availableThinkingLevels, levels)
  assert.equal(state.thinkingStatus, 'supported')
  assert.equal(state.status, 'ok')
  assert.equal(state.supported, levels.length > 1)
  assert.equal(state.message, '')
  assert.equal(state.thinkingMessage, '')
}

async function sessionPath(id) {
  const directory = join(agent, 'sessions')
  for (const path of await readdir(directory, { recursive: true })) {
    if (!path.endsWith('.jsonl')) continue
    const candidate = join(directory, path)
    const header = JSON.parse((await readFile(candidate, 'utf8')).split('\n')[0])
    if (header.id === id) return candidate
  }
  assert.fail('Fixture session must be persisted')
}

try {
  await Promise.all([agent, workspace].map((path) => mkdir(path)))
  const models = ['fixed', 'reasoner'].map((id) => ({
    id,
    name: id,
    reasoning: id === 'reasoner',
    input: ['text'],
    contextWindow: 32000,
    maxTokens: 1000,
  }))
  for (const [name, data] of Object.entries({
    'models.json': {
      providers: {
        [provider]: { api: 'openai-completions', baseUrl: 'http://127.0.0.1:1/v1', models },
      },
    },
    'auth.json': { [provider]: { type: 'api_key', key: 'synthetic-thinking-key' } },
    'settings.json': {
      defaultProvider: provider,
      defaultModel: 'fixed',
      defaultThinkingLevel: 'medium',
    },
    'pisper.json': {},
  }))
    await writeFile(join(agent, name), JSON.stringify(data), { mode: 0o600 })

  await start()
  const created = await json('/api/sessions', 'POST', { name: 'Thinking fixture' }, 201)
  const id = created.id
  const endpoint = `/api/sessions/${id}`
  verifyState(await json(`${endpoint}/thinking-level`), id, 'fixed', 'off', ['off'])

  const switched = await json(`${endpoint}/model`, 'PUT', { provider, model: 'reasoner' })
  verifyState(switched, id, 'reasoner', 'medium', reasoningLevels)
  assert.equal(switched.ok, true)
  assert.equal(switched.provider, provider)
  assert.equal(switched.modelId, 'reasoner')
  assert.ok(Object.hasOwn(switched, 'contextUsage'))
  verifyState(await json(`${endpoint}/thinking-level`), id, 'reasoner', 'medium', reasoningLevels)
  verifyState(
    await json(`${endpoint}/thinking-level`, 'PUT', { level: 'high' }),
    id,
    'reasoner',
    'high',
    reasoningLevels,
  )

  const path = await sessionPath(id)
  const beforeRejected = await readFile(path, 'utf8')
  const rejected = await json(`${endpoint}/thinking-level`, 'PUT', { level: 'max' }, 400)
  assert.equal(rejected.code, 'unsupported_thinking_level')
  const malformed = await json(`${endpoint}/thinking-level`, 'POST', { level: 'invalid' }, 400)
  assert.equal(malformed.code, 'bad_request')
  verifyState(await json(`${endpoint}/thinking-level`), id, 'reasoner', 'high', reasoningLevels)
  assert.equal(
    await readFile(path, 'utf8'),
    beforeRejected,
    'Rejected changes must not rewrite history',
  )

  await stop()
  await start()
  verifyState(await json(`${endpoint}/thinking-level`), id, 'reasoner', 'high', reasoningLevels)
  assert.equal(await readFile(path, 'utf8'), beforeRejected, 'Cold reads must preserve history')
  verifyState(
    await json(`${endpoint}/model`, 'POST', { provider, model: 'fixed' }),
    id,
    'fixed',
    'off',
    ['off'],
  )
  const fixedBeforeRejected = await readFile(path, 'utf8')
  assert.equal(
    (await json(`${endpoint}/thinking-level`, 'PUT', { level: 'medium' }, 400)).code,
    'unsupported_thinking_level',
  )
  assert.equal(await readFile(path, 'utf8'), fixedBeforeRejected)
  verifyState(
    await json(`${endpoint}/thinking-level`, 'POST', { level: 'off' }),
    id,
    'fixed',
    'off',
    ['off'],
  )

  await stop()
  // 模拟旧会话保留了后来被模型配置禁用的档位；只读恢复必须归一展示，不迁移文件。
  const entries = (await readFile(path, 'utf8')).trim().split('\n').map(JSON.parse)
  await appendFile(
    path,
    JSON.stringify({
      type: 'thinking_level_change',
      id: 'legacy-thinking-level',
      parentId: entries.at(-1).id,
      timestamp: new Date().toISOString(),
      thinkingLevel: 'medium',
    }) + '\n',
  )
  const legacyBytes = await readFile(path, 'utf8')
  await start()
  verifyState(await json(`${endpoint}/thinking-level`), id, 'fixed', 'off', ['off'])
  assert.equal(await readFile(path, 'utf8'), legacyBytes)
  console.log(
    'PASS thinking contract: model response, capability aliases, effective levels, rejected writes, cold reads and restart',
  )
} finally {
  await stop()
  await rm(sandbox, { recursive: true, force: true })
}
