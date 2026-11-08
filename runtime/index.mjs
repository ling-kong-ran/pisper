// 开发/直跑入口：与 sidecar.mjs 不同，本入口默认使用 Vite 中间件托管前端（npm run dev），
// 并以 127.0.0.1:5173 提供 API。桌面端走 sidecar.mjs，不经过这里。
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createPisperRuntime } from './app-runtime.mjs'
import { resolveAgentDataDir } from './data-dir-migration.mjs'
import { openBrowser, shouldOpenBrowser } from './open-browser.mjs'

// 关闭 Pi 引擎自带的版本检查与遥测：这些行为由 Pisper 自己控制，避免重复或干扰。
process.env.PI_SKIP_VERSION_CHECK ||= '1'
process.env.PI_TELEMETRY ||= '0'

const serverDir = dirname(fileURLToPath(import.meta.url))
const root = resolve(serverDir, '..')
const dataDir = resolveAgentDataDir()
const production = process.argv.includes('--production')
const port = Number(process.env.PORT || 5173)
const host = process.env.HOST || '127.0.0.1'
// --remote / PISPER_REMOTE=1：启用远程访问（局域网 HTTPS + 设备配对），供移动端连接。
const remoteFlag = process.argv.includes('--remote') || process.env.PISPER_REMOTE === '1'

const pisper = await createPisperRuntime({
  root,
  runtimeCwd: process.env.PISPER_WORKSPACE_DIR || undefined,
  dataDir,
  production,
  port,
  host,
  remote: remoteFlag ? { enabled: true } : { enabled: false },
})
console.log('')
console.log(`Pisper is running: ${pisper.url}`)
console.log(`Data folder: ${pisper.dataDir}`)
const remoteStatus = pisper.remoteControl.status()
if (remoteStatus.listening) {
  console.log('Remote access is on; phones can connect at:')
  for (const endpoint of remoteStatus.endpoints) console.log(`  [${endpoint.t}] ${endpoint.url}`)
  console.log(`Certificate fingerprint: ${remoteStatus.fingerprint}`)
  console.log('Create a pairing QR code in Settings → Remote access.')
} else if (remoteStatus.enabled) {
  console.log(`Remote access failed to start: ${remoteStatus.error || 'unknown reason'}`)
}
// 仅在显式设置 PISPER_OPEN_BROWSER=1 且非 CI 环境时自动打开浏览器，
// 避免在无人值守/远程场景下意外拉起浏览器窗口。
if (shouldOpenBrowser({ host })) {
  const opening = openBrowser(pisper.url)
  console.log(opening ? 'Opening the default browser…' : 'Could not open the browser.')
}
console.log(`Open in your browser: ${pisper.url}`)
console.log('Set PISPER_OPEN_BROWSER=1 to open the browser on start.')
console.log('Press Ctrl+C to stop Pisper.')
console.log('')

// 优雅退出：信号可能重复触发（如连续两次 Ctrl+C），用标志位保证只执行一次清理。
let shuttingDown = false
async function shutdown() {
  if (shuttingDown) return
  shuttingDown = true
  await pisper.close()
  process.exit(0)
}

process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
