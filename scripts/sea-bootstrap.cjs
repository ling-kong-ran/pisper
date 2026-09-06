const { existsSync } = require('node:fs')
const { dirname, join, resolve } = require('node:path')
const { pathToFileURL } = require('node:url')

const executableDirectory = dirname(process.execPath)
const appRoot = resolve(process.env.PISPER_APP_ROOT || join(executableDirectory, 'runtime'))
// 语音子进程只加载推理入口，不能再次启动 HTTP、Agent 或桌面握手。
const speechWorker = process.argv.includes('--pisper-speech-worker')
if (speechWorker && typeof process.send !== 'function') {
  console.error('Speech worker requires a parent IPC channel.')
  process.exit(1)
}
const entrypoint = speechWorker
  ? join(appRoot, 'runtime', 'workers', 'speech-inference-worker.mjs')
  : join(appRoot, 'runtime', 'sidecar.mjs')

if (!existsSync(entrypoint)) {
  console.error(`Pisper sidecar runtime was not found at ${entrypoint}`)
  process.exit(1)
}

process.env.PISPER_APP_ROOT = appRoot
import(pathToFileURL(entrypoint).href).catch((error) => {
  console.error(error)
  process.exit(1)
})
