// 真实 CLI 可执行文件验证；隔离 HOME 和代理目录，不访问用户会话或模型。
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, realpath, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve, toNamespacedPath } from 'node:path'

const root = resolve(import.meta.dirname, '..')
const cli = resolve(
  process.argv[2] ||
    join(root, 'src-tui/target/debug', process.platform === 'win32' ? 'pisper.exe' : 'pisper'),
)
const sandbox = await mkdtemp(join(await realpath(tmpdir()), 'pisper-cli-smoke-'))
const agent = join(sandbox, 'agent')
const workspace = join(sandbox, 'workspace with spaces')
const canonicalWorkspace = toNamespacedPath(workspace)
const environment = {
  ...Object.fromEntries(
    Object.entries(process.env).filter(([key]) =>
      /^(PATH|PATHEXT|SYSTEMROOT|WINDIR|TEMP|TMP)$/i.test(key),
    ),
  ),
  HOME: sandbox,
  USERPROFILE: sandbox,
  LOCALAPPDATA: sandbox,
  APPDATA: sandbox,
  XDG_DATA_HOME: sandbox,
  PISPER_AGENT_DIR: agent,
  PI_CODING_AGENT_DIR: agent,
  PISPER_RS_DATA_DIR: agent,
  PI_SKIP_VERSION_CHECK: '1',
  PI_TELEMETRY: '0',
}
const report = { checks: [], timings: [] }
async function run(args, extra = {}, expected = 0) {
  const started = performance.now()
  const child = spawn(cli, args, {
    cwd: workspace,
    env: { ...environment, ...extra },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  let output = ''
  child.stdout.on('data', (chunk) => {
    output += chunk
  })
  child.stderr.on('data', (chunk) => {
    output += chunk
  })
  const timer = setTimeout(() => child.kill('SIGKILL'), 30000)
  try {
    const code = await new Promise((resolveExit, reject) => {
      child.once('error', reject)
      child.once('close', resolveExit)
    })
    assert.equal(code, expected, output.replaceAll(sandbox, '<fixture>'))
    report.timings.push({ command: args[0], milliseconds: Math.round(performance.now() - started) })
    return output
  } finally {
    clearTimeout(timer)
  }
}
try {
  await Promise.all([agent, workspace].map((dir) => mkdir(dir)))
  assert.match(await run(['--help']), /doctor/)
  assert.match(await run([], {}, 1), /requires an interactive terminal/)
  assert.match(await run(['help', 'web']), /web/)
  const first = await run(['doctor', '--cwd', workspace])
  assert.match(first, /Pisper TUI ready/)
  assert.match(first, /connection: TUI sidecar/)
  assert.ok(first.includes(`launch workspace: ${canonicalWorkspace}`))
  assert.ok(first.includes(`runtime fallback: ${canonicalWorkspace}`))
  const second = await run(['doctor', '--cwd', canonicalWorkspace])
  assert.match(second, /Pisper TUI ready/)
  // 两次独立启动能退出并重开同一个隔离存储，覆盖 Runtime 清理与恢复。
  report.checks.push(
    'help, Rust runtime discovery, workspace binding, doctor, shutdown and restart',
  )
  assert.match(
    await run(['doctor'], { PISPER_TUI_URL: 'http://127.0.0.1:1' }, 1),
    /PISPER_TUI_TOKEN is required/,
  )
  assert.match(await run(['doctor', '--cwd', join(sandbox, 'missing')], {}, 1), /directory|目录/i)
  report.checks.push('missing remote token and invalid workspace fail with actionable errors')
  // 验证测试没有悄悄执行安装器，形成新的旧 Runtime 组件。
  await assert.rejects(
    readFile(join(sandbox, 'com.lingkongran.pisper/components/runtime/current.json')),
    { code: 'ENOENT' },
  )
  console.log(JSON.stringify(report, null, 2))
} finally {
  await rm(sandbox, { recursive: true, force: true })
}
