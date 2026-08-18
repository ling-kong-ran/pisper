// 桌面端 Sidecar 入口（Node SEA 打包目标）：由 Tauri 壳进程拉起，通过 stdin/环境变量与父进程通信。
// 与 index.mjs（开发 Web 入口）的区别：这里总是生产模式、不托管 Vite、
// 监听随机端口（port: 0），并以一次性桌面令牌鉴权后交给 Tauri WebView 使用。
import { randomBytes } from 'node:crypto'
import { dirname, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { createPisperRuntime } from './app-runtime.mjs'
import { resolveAgentDataDir } from './data-dir-migration.mjs'

const serverDir = dirname(fileURLToPath(import.meta.url))
const defaultRoot = resolve(serverDir, '..')
// PISPER_APP_ROOT 允许 Tauri 壳把运行时指向打包的 sidecar-runtime 目录。
const root = resolve(process.env.PISPER_APP_ROOT || defaultRoot)
const host = '127.0.0.1'
// 未显式注入令牌时自生成随机令牌，作为桌面 WebView 访问 API 的凭据。
const token = String(process.env.PISPER_DESKTOP_TOKEN || randomBytes(32).toString('base64url'))
// 父进程（Tauri）PID：父进程退出时本进程随之退出，避免残留孤儿进程。
const parentPid = Number(process.env.PISPER_PARENT_PID || 0)
let previousStageAt = 0
const startupTiming = process.env.PISPER_STARTUP_TIMING === '1'

// 启动阶段打点：写入 stderr 而非 stdout（stdout 保留给 READY 消息），
// 便于 Tauri 侧统计各阶段耗时，定位启动瓶颈。
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
// 统一退出路径：先关闭运行时代码，再以指定退出码结束进程；重复调用仅生效一次。
async function shutdown(code = 0) {
  if (shuttingDown) return
  shuttingDown = true
  try {
    await pisper?.close()
  } finally {
    process.exit(code)
  }
}

// deferRuntimeInitialization 让 HTTP 先监听、运行时后初始化：WebView 可以尽早拿到 READY 消息；
// 运行时的真实初始化进度通过 initialized Promise 上报，失败时整体退出。
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

// 启动引导 URL：Tauri 侧打开它完成令牌 Cookie 植入，之后 API 请求即通过鉴权。
const bootstrapUrl = `${pisper.url}/_pisper/desktop/bootstrap?token=${encodeURIComponent(token)}`
reportStartupStage('ready')
// READY 消息是 Tauri 壳与 sidecar 之间的握手协议，行格式必须保持稳定。
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

// 父进程可通过 stdin 发送 `shutdown` 命令，或关闭 stdin 触发退出（stdin 关闭即父进程消失）。
const input = createInterface({ input: process.stdin, terminal: false })
input.on('line', (line) => {
  if (line.trim().toLowerCase() === 'shutdown') void shutdown(0)
})
if (process.env.PISPER_EXIT_ON_STDIN_CLOSE === '1') input.on('close', () => void shutdown(0))

// 心跳监控父进程：signal 0 仅探测存在性；父进程消失则自行退出。
// unref 保证定时器不阻止进程退出。
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
// 未捕获异常/拒绝必须记录并带错误码退出，避免 sidecar 处于“半死”状态被桌面误认为健康。
process.on('uncaughtException', (error) => {
  console.error(error)
  void shutdown(1)
})
process.on('unhandledRejection', (error) => {
  console.error(error)
  void shutdown(1)
})
