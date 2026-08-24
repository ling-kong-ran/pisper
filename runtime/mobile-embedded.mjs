// 嵌入式 Node 与 App 同进程，不能调用 process.exit。壳层通过权限收紧的 READY
// 文件取得认证入口；Runtime 保持驻留，直到操作系统结束 App 进程。
import { randomBytes } from 'node:crypto'
import { chmod, mkdir, rename, writeFile } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createPisperRuntime } from './app-runtime.mjs'
import { resolveAgentDataDir } from './data-dir-migration.mjs'

const serverDir = dirname(fileURLToPath(import.meta.url))

async function report(readyFile, value) {
  const temporary = `${readyFile}.tmp`
  await mkdir(dirname(readyFile), { recursive: true })
  await writeFile(temporary, `${JSON.stringify(value)}\n`, { mode: 0o600 })
  await chmod(temporary, 0o600)
  await rename(temporary, readyFile)
}

export async function startEmbeddedRuntime() {
  const root = resolve(process.env.PISPER_APP_ROOT || resolve(serverDir, '..'))
  const readyFile = String(process.env.PISPER_MOBILE_READY_FILE || '').trim()
  const token = String(process.env.PISPER_DESKTOP_TOKEN || randomBytes(32).toString('base64url'))
  if (!readyFile) throw new Error('PISPER_MOBILE_READY_FILE is required')
  process.env.PI_SKIP_VERSION_CHECK ||= '1'
  process.env.PI_TELEMETRY ||= '0'

  let pisper
  try {
    pisper = await createPisperRuntime({
      root,
      runtimeCwd: process.env.PISPER_WORKSPACE_DIR || undefined,
      dataDir: resolveAgentDataDir(),
      production: true,
      port: 0,
      host: '127.0.0.1',
      desktopAuthToken: token,
      frontendRoot: process.env.PISPER_FRONTEND_ROOT || resolve(root, 'dist'),
      deferRuntimeInitialization: true,
    })
    await pisper.initialized
    await report(readyFile, {
      url: pisper.url,
      bootstrapUrl: `${pisper.url}/_pisper/desktop/bootstrap?token=${encodeURIComponent(token)}`,
      pid: process.pid,
      runtimeProfile:
        process.env.PISPER_RUNTIME_PROFILE === 'mobile-store' ? 'mobile-store' : 'mobile-embedded',
    })
    return pisper
  } catch (error) {
    await pisper?.close()
    throw error
  }
}

const shouldAutoStart =
  process.env.PISPER_MOBILE_AUTOSTART === '1' ||
  (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url))

if (shouldAutoStart) {
  try {
    await startEmbeddedRuntime()
  } catch (error) {
    console.error('Pisper embedded runtime initialization failed.', error)
    const readyFile = String(process.env.PISPER_MOBILE_READY_FILE || '').trim()
    if (readyFile) {
      await report(readyFile, { error: error instanceof Error ? error.message : String(error) })
    }
  }
}
