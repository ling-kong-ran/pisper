import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createPisperServer } from './app-server.mjs'
import { resolveAgentDataDir } from './data-dir-migration.mjs'
import { openBrowser, shouldOpenBrowser } from './open-browser.mjs'

process.env.PI_SKIP_VERSION_CHECK ||= '1'
process.env.PI_TELEMETRY ||= '0'

const serverDir = dirname(fileURLToPath(import.meta.url))
const root = resolve(serverDir, '..')
const dataDir = resolveAgentDataDir()
const production = process.argv.includes('--production')
const port = Number(process.env.PORT || 5173)
const host = process.env.HOST || '127.0.0.1'

const pisper = await createPisperServer({
  root,
  runtimeCwd: process.env.PISPER_WORKSPACE_DIR || undefined,
  dataDir,
  production,
  port,
  host,
})
console.log('')
console.log(`Pisper 已启动：${pisper.url}`)
console.log(`数据目录：${pisper.dataDir}`)
if (shouldOpenBrowser({ host })) {
  const opening = openBrowser(pisper.url)
  console.log(opening ? '正在打开默认浏览器…' : '未能自动打开浏览器。')
}
console.log(`请在浏览器中访问：${pisper.url}`)
console.log('如需启动时自动打开浏览器，可设置 PISPER_OPEN_BROWSER=1。')
console.log('按 Ctrl+C 停止 Pisper。')
console.log('')

let shuttingDown = false
async function shutdown() {
  if (shuttingDown) return
  shuttingDown = true
  await pisper.close()
  process.exit(0)
}

process.on('SIGINT', shutdown)
process.on('SIGTERM', shutdown)
