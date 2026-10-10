// 隔离进程验证会话读取缓存；只生成虚构历史，不使用个人目录或外部模型。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import {
  mkdir,
  mkdtemp,
  readFile,
  writeFile,
  appendFile,
  rename,
  stat,
  utimes,
  rm,
  realpath,
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'

const root = resolve(import.meta.dirname, '..')
const executable = resolve(process.argv[2] || join(root, 'runtime-rs/target/debug/pisper-server'))
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-session-reads-'))
const agent = join(sandbox, 'agent')
const workspace = join(sandbox, 'workspace')
const sessions = join(agent, 'sessions')
const token = 'synthetic-session-read-token'
const report = { checks: [], timings: [] }
let child, ended, base
const created = '2026-10-11T00:00:00.000Z'
const entry = (id, parentId, text) => ({
  type: 'message',
  id,
  parentId,
  timestamp: created,
  message: { role: 'user', timestamp: 1791676800000, content: text },
})
const header = (id) => ({ type: 'session', version: 3, id, cwd: workspace, timestamp: created })
const encode = (rows) => rows.map((row) => JSON.stringify(row)).join('\n') + '\n'
async function api(path, { method = 'GET', body, status = 200, measure } = {}) {
  const start = performance.now()
  const response = await fetch(base + path, {
    method,
    headers: { Cookie: `__pisper_desktop=${token}`, 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(30000),
  })
  assert.equal(response.status, status, `${method} ${path}: status`)
  const value = await response.json()
  if (measure)
    report.timings.push({ name: measure, milliseconds: Math.round(performance.now() - start) })
  return value
}
async function start() {
  let output = '',
    diagnostics = '',
    failure
  child = spawn(executable, [], {
    cwd: workspace,
    env: {
      ...Object.fromEntries(
        Object.entries(process.env).filter(([key]) =>
          /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP)$/i.test(key),
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
  ended = new Promise((resolveExit) => child.once('exit', resolveExit))
  child.on('error', (error) => {
    failure = error
  })
  child.stderr.on('data', (chunk) => {
    diagnostics = (diagnostics + chunk).slice(-2000)
  })
  child.stdout.on('data', (chunk) => {
    output += chunk
    const line = output.split('\n').find((line) => line.startsWith('PISPER_SIDECAR_READY '))
    if (line) base = JSON.parse(line.slice('PISPER_SIDECAR_READY '.length)).url
  })
  const deadline = Date.now() + 30000
  while (!base && !failure && child.exitCode === null && Date.now() < deadline) await delay(50)
  assert.ok(
    base && !failure,
    `Session fixture startup failed: ${diagnostics.replaceAll(sandbox, '<fixture>').replaceAll(token, '<token>')}`,
  )
}
async function stop() {
  if (child && child.exitCode === null) {
    child.stdin.end('shutdown\n')
    let forced = false
    const timer = setTimeout(() => {
      forced = true
      child.kill('SIGKILL')
    }, 15000)
    try {
      await ended
      assert.equal(forced, false, 'Session fixture must shut down cleanly')
    } finally {
      clearTimeout(timer)
    }
  }
}
try {
  await Promise.all([sessions, workspace].map((path) => mkdir(path, { recursive: true })))
  const smallPath = join(sessions, 'small.jsonl')
  const smallOriginal = encode([header('small'), entry('small-one', null, 'before')])
  await writeFile(smallPath, smallOriginal)
  const largePath = join(sessions, 'large.jsonl')
  const largeRows = [header('large')]
  for (let i = 0; i < 160; i++)
    largeRows.push(entry(`large-${i}`, i ? `large-${i - 1}` : null, `${i}:` + 'x'.repeat(32768)))
  const largeOriginal = encode(largeRows)
  await writeFile(largePath, largeOriginal)
  await start()
  // 配置写入也不能丢失并发修改：多个技能开关共用同一个持久化文件。
  for (let i = 0; i < 12; i++) {
    const dir = join(agent, 'skills', `concurrent-${i}`)
    await mkdir(dir, { recursive: true })
    await writeFile(
      join(dir, 'SKILL.md'),
      `---\nname: concurrent-${i}\ndescription: Synthetic concurrency probe\n---\nfixture`,
    )
  }
  const skills = (await api('/api/skills')).skills.filter((skill) =>
    skill.name.startsWith('concurrent-'),
  )
  assert.equal(skills.length, 12)
  const updates = await Promise.allSettled(
    skills.flatMap((skill) => [
      api(`/api/skills/${skill.id}`, { method: 'PATCH', body: { enabled: false } }),
      api(`/api/skills/${skill.id}`, { method: 'PATCH', body: { modelInvocationEnabled: false } }),
    ]),
  )
  assert.ok(
    updates.every((result) => result.status === 'fulfilled'),
    'Concurrent skill updates must all succeed',
  )
  for (const skill of (await api('/api/skills')).skills.filter((skill) =>
    skill.name.startsWith('concurrent-'),
  )) {
    assert.equal(skill.enabled, false)
    assert.equal(skill.modelInvocationEnabled, false)
  }
  report.checks.push('concurrent skill preferences preserve both fields and every skill')
  const first = await api('/api/sessions/small/messages?limit=40', { measure: 'small-cold' })
  assert.equal(first.messages[0].text, 'before')
  assert.deepEqual(
    await api('/api/sessions/small/messages?limit=40', { measure: 'small-warm' }),
    first,
  )
  const latest = await api('/api/sessions/large/messages?limit=40', { measure: 'large-cold' })
  const earlier = await api(
    `/api/sessions/large/messages?limit=40&before=${latest.pageInfo.nextCursor}`,
  )
  assert.equal(latest.messages.length, 40)
  assert.equal(earlier.pageInfo.end, latest.pageInfo.start)
  assert.ok(
    earlier.messages.every((message) => !latest.messages.some((later) => later.id === message.id)),
  )
  assert.deepEqual(
    await api('/api/sessions/large/messages?limit=40', { measure: 'large-warm' }),
    latest,
  )
  assert.equal(await readFile(smallPath, 'utf8'), smallOriginal)
  assert.equal(await readFile(largePath, 'utf8'), largeOriginal)
  await mkdir(join(workspace, '.pisper/prompts'), { recursive: true })
  await writeFile(
    join(workspace, '.pisper/prompts/cache-probe.md'),
    '---\ndescription: Fixture command\n---\nfixture',
  )
  await mkdir(join(workspace, '.pisper/skills/project-probe'), { recursive: true })
  await writeFile(
    join(workspace, '.pisper/skills/project-probe/SKILL.md'),
    '---\nname: project-probe\ndescription: Fixture skill\n---\nfixture',
  )
  const commands = await api('/api/sessions/large/commands', { measure: 'large-session-commands' })
  assert.ok(JSON.stringify(commands).includes('/cache-probe'))
  assert.ok(JSON.stringify(commands).includes('/skill:project-probe'))
  report.checks.push('unchanged reads and pagination preserve exact messages and source files')

  await appendFile(smallPath, encode([entry('small-two', 'small-one', 'appended')]))
  const appended = await api('/api/sessions/small/messages?limit=40')
  assert.equal(appended.messages.at(-1).text, 'appended')
  const live = await api('/api/sessions/small/live')
  assert.equal(live.messages.at(-1).text, 'appended')
  report.checks.push('external append invalidates messages and live projection together')

  await api('/api/sessions/small', { method: 'PATCH', body: { name: 'renamed' } })
  let list = await api('/api/sessions', { measure: 'list-cold' })
  assert.equal(list.sessions.find((row) => row.id === 'small').name, 'renamed')
  await api('/api/sessions', { measure: 'list-warm' })
  const beforeStat = await stat(smallPath)
  const content = await readFile(smallPath, 'utf8')
  const replacement = join(sessions, 'replacement.tmp')
  await writeFile(replacement, content.replace('before', 'after!'))
  await utimes(replacement, beforeStat.atime, beforeStat.mtime)
  await rename(replacement, smallPath)
  assert.equal((await api('/api/sessions/small/messages?limit=40')).messages[0].text, 'after!')
  report.checks.push('rename and atomic same-size replacement invalidate cached values')

  const second = join(workspace, 'second')
  await mkdir(second)
  await api('/api/sessions/small/cwd', { method: 'PUT', body: { cwd: second } })
  assert.equal((await api('/api/sessions/small/live')).cwd, second)
  assert.equal((await api('/api/sessions/small/file-changes')).cwd, second)
  report.checks.push('cwd changes refresh both live metadata and file-change directory')

  const markedPath = join(sessions, 'marked.jsonl')
  const markedOriginal = encode([
    header('marked'),
    {
      type: 'model_change',
      id: 'missing-model',
      parentId: null,
      timestamp: created,
      provider: 'unconfigured-fixture',
      modelId: 'missing',
    },
    entry('marked-user', 'missing-model', 'question'),
    {
      type: 'message',
      id: 'answer',
      parentId: 'marked-user',
      timestamp: created,
      message: {
        role: 'assistant',
        content: [{ type: 'text', text: 'bookmark answer' }],
        api: 'openai-completions',
        provider: 'unconfigured-fixture',
        model: 'missing',
        stopReason: 'stop',
        timestamp: 1791676801000,
        usage: {
          input: 1,
          output: 1,
          cacheRead: 0,
          cacheWrite: 0,
          totalTokens: 2,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
        },
      },
    },
  ])
  await writeFile(markedPath, markedOriginal)
  assert.equal((await api('/api/session-labels?limit=500')).labels.length, 0)
  const tree = await api('/api/sessions/marked/tree/labels/answer', {
    method: 'PUT',
    body: { label: 'first mark' },
    measure: 'cold-mark',
  })
  assert.equal(tree.nodes.find((node) => node.id === 'answer').label, 'first mark')
  const appendedRows = (await readFile(markedPath, 'utf8'))
    .slice(markedOriginal.length)
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line))
  assert.deepEqual(
    appendedRows.map((row) => row.type),
    ['label'],
    'Marking must not bootstrap an engine or append model defaults',
  )
  assert.equal(
    (await api('/api/session-labels?limit=500', { measure: 'all-marks-cold' })).labels[0].label,
    'first mark',
  )
  assert.equal(
    (await api('/api/session-labels?limit=500', { measure: 'all-marks-warm' })).labels[0].label,
    'first mark',
  )
  await Promise.all([
    api('/api/sessions/marked/tree/labels/answer', { method: 'PUT', body: { label: 'updated' } }),
    api('/api/sessions/marked', { method: 'PATCH', body: { name: 'marked title' } }),
  ])
  const marks = await api('/api/session-labels?query=updated')
  assert.equal(marks.labels[0].sessionName, 'marked title')
  assert.equal(marks.labels[0].label, 'updated')
  await api('/api/sessions/marked/tree/labels/answer', { method: 'PUT', body: { label: '' } })
  assert.equal((await api('/api/session-labels?limit=500')).labels.length, 0)
  await api('/api/sessions/marked/tree/labels/missing', {
    method: 'PUT',
    body: { label: 'invalid' },
    status: 400,
  })
  report.checks.push(
    'cold marks and rename need no configured model; concurrent edits, tombstones and cached all-marks remain consistent',
  )

  await rm(smallPath)
  await api('/api/sessions/small/messages?limit=40', { status: 404 })
  list = await api('/api/sessions')
  assert.ok(!list.sessions.some((row) => row.id === 'small'))
  await writeFile(
    smallPath,
    encode([header('replacement'), entry('replacement-one', null, 'replacement')]),
  )
  assert.equal(
    (await api('/api/sessions/replacement/messages?limit=40')).messages[0].text,
    'replacement',
  )
  await api('/api/sessions/small/live', { status: 404 })
  report.checks.push('deletion and recreation never revive an old ID')
  console.log(JSON.stringify({ status: 'passed', ...report }, null, 2))
} finally {
  await stop()
  await rm(sandbox, { recursive: true })
}
