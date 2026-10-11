// 仅针对本任务的独立 AVD：真实 APK 生命周期 + 同源 HTTP 合同，绝不使用真实手机数据。
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdir, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { resolveAndroidEnv } from './android-env.mjs'

const root = resolve(import.meta.dirname, '..')
const env = resolveAndroidEnv()
const adbPath = join(
  env.ANDROID_HOME,
  'platform-tools',
  process.platform === 'win32' ? 'adb.exe' : 'adb',
)
const serial = process.argv[2]
if (!/^emulator-\d+$/.test(serial || '')) throw new Error('Specify an isolated emulator serial')
const adb = (...args) =>
  execFileSync(adbPath, ['-s', serial, ...args], { encoding: 'utf8', timeout: 30000 }).trim()
assert.equal(adb('emu', 'avd', 'name').split(/\r?\n/)[0], 'pisper-rust-release-validation')
const application = 'com.lingkongran.pisper'
const output = join(root, 'release/android-emulator')
await mkdir(output, { recursive: true })
const report = { checks: [] }
let connection
async function connect() {
  const pid = adb('shell', 'pidof', application)
  assert.match(pid, /^\d+$/)
  const port = adb('forward', 'tcp:0', `localabstract:webview_devtools_remote_${pid}`)
  const targets = await fetch(`http://127.0.0.1:${port}/json/list`).then((r) => r.json())
  assert.equal(targets.length, 1)
  const ws = new WebSocket(targets[0].webSocketDebuggerUrl)
  await new Promise((resolve, reject) => {
    ws.addEventListener('open', resolve, { once: true })
    ws.addEventListener('error', reject, { once: true })
  })
  let nextId = 0
  const pending = new Map()
  const exceptions = []
  ws.addEventListener('message', (event) => {
    const result = JSON.parse(event.data)
    if (result.method === 'Runtime.exceptionThrown') {
      const detail = result.params.exceptionDetails
      exceptions.push(detail.exception?.description?.split('\n')[0] || detail.text)
      return
    }
    const call = pending.get(result.id)
    if (!call) return
    pending.delete(result.id)
    clearTimeout(call.timer)
    if (result.error) call.reject(new Error(result.error.message))
    else call.resolve(result.result)
  })
  const call = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++nextId
      const timer = setTimeout(() => {
        pending.delete(id)
        reject(new Error(`${method} timed out`))
      }, 10000)
      pending.set(id, { resolve, reject, timer })
      ws.send(JSON.stringify({ id, method, params }))
    })
  await call('Runtime.enable')
  return {
    pid,
    call,
    exceptions,
    close() {
      ws.close()
      adb('forward', '--remove', `tcp:${port}`)
    },
  }
}
async function waitReady() {
  const deadline = Date.now() + 60000
  let last = 'not started'
  while (Date.now() < deadline) {
    try {
      connection = await connect()
      const { result } = await connection.call('Runtime.evaluate', {
        expression:
          'JSON.stringify({url:location.href,title:document.title,text:document.body.innerText,inputs:document.querySelectorAll("textarea").length})',
        returnByValue: true,
      })
      const page = JSON.parse(result.value)
      const origin = new URL(page.url)
      if (
        origin.hostname === '127.0.0.1' &&
        !origin.pathname.includes('startup') &&
        page.inputs > 0
      )
        return { page, origin }
      last = page.text.slice(0, 300)
    } catch (error) {
      last = error.message
    }
    connection?.close()
    connection = null
    await new Promise((resolve) => setTimeout(resolve, 500))
  }
  throw new Error(`Android did not become ready: ${last}`)
}
async function apiFor(origin) {
  const { cookies } = await connection.call('Network.getAllCookies')
  const port = adb('forward', 'tcp:0', `tcp:${origin.port}`)
  const base = `http://127.0.0.1:${port}`
  const cookie = cookies.map(({ name, value }) => `${name}=${value}`).join('; ')
  return {
    async json(path, method = 'GET', body) {
      const response = await fetch(base + path, {
        method,
        headers: {
          Host: origin.host,
          Origin: origin.origin,
          Cookie: cookie,
          'Content-Type': 'application/json',
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        signal: AbortSignal.timeout(20000),
      })
      assert(response.ok, `${method} ${path}: HTTP ${response.status}`)
      if (response.status === 204) return null
      return response.json()
    },
    close() {
      adb('forward', '--remove', `tcp:${port}`)
    },
  }
}
let api
try {
  const first = await waitReady()
  assert(first.page.inputs > 0, 'React composer must render')
  api = await apiFor(first.origin)
  const ready = await api.json('/api/ready')
  assert.equal(ready.profile, 'mobile-embedded')
  const capabilities = await api.json('/api/runtime/capabilities')
  assert.equal(capabilities.engine, 'pi-rs')
  assert.equal(capabilities.features.shell, false)
  report.checks.push(
    'release APK cold-starts authenticated Rust runtime and renders the React composer',
  )
  const session = await api.json('/api/sessions', 'POST', {
    name: 'Android validation fixture',
  })
  assert(session?.id)
  const previousPreferences = await api.json('/api/local/browser-preferences')
  await api.json('/api/local/browser-preferences', 'PUT', {
    updates: { 'pisper-language': 'en-US' },
    revisions: {
      'pisper-language': Math.max(
        Date.now() * 1000,
        (previousPreferences.revisions['pisper-language'] || 0) + 1,
      ),
    },
  })
  assert.equal(
    (await api.json('/api/local/browser-preferences')).values['pisper-language'],
    'en-US',
  )
  const config = await api.json('/api/config')
  assert(Array.isArray(config.providers))
  const providerId = `android-fixture-${Date.now().toString(36)}`
  await api.json('/api/providers', 'POST', {
    id: providerId,
    name: 'Android validation provider',
    api: 'openai-completions',
    baseUrl: 'http://127.0.0.1:9/v1',
    apiKey: 'synthetic-validation-key',
    model: 'fixture-chat',
  })
  const saved = await api.json('/api/config')
  assert(
    saved.providers.some(
      (provider) =>
        provider.id === providerId && provider.models.some((model) => model.id === 'fixture-chat'),
    ),
  )
  assert(!JSON.stringify(saved).includes('synthetic-validation-key'))
  const selected = await api.json(`/api/sessions/${session.id}/model`, 'PUT', {
    provider: providerId,
    model: 'fixture-chat',
  })
  assert.equal(selected.modelId, 'fixture-chat')
  report.checks.push(
    'session creation, configuration read and page preferences work through the native proxy',
  )
  api.close()
  api = null
  connection.close()
  connection = null
  adb('shell', 'am', 'force-stop', application)
  adb('shell', 'am', 'start', '-W', '-n', `${application}/.MainActivity`)
  const second = await waitReady()
  api = await apiFor(second.origin)
  const restored = await api.json('/api/sessions')
  assert(restored.sessions.some((item) => item.id === session.id))
  assert.equal((await api.json(`/api/sessions/${session.id}/model`)).id, 'fixture-chat')
  const savedConfig = await api.json('/api/config')
  assert(
    savedConfig.providers.some(
      (provider) =>
        provider.id === providerId && provider.models.some((model) => model.id === 'fixture-chat'),
    ),
  )
  const preferences = await api.json('/api/local/browser-preferences')
  assert.equal(preferences.values['pisper-language'], 'en-US')
  report.checks.push(
    'force-stop and cold restart retain sessions and preferences under the original data directory',
  )
  const pid = connection.pid
  adb(
    'shell',
    'am',
    'start',
    '-a',
    'android.intent.action.MAIN',
    '-c',
    'android.intent.category.HOME',
  )
  adb('shell', 'am', 'start', '-W', '-n', `${application}/.MainActivity`)
  assert.equal(adb('shell', 'pidof', application), pid)
  assert.equal((await api.json('/api/ready')).ready, true)
  report.checks.push('background and foreground preserve the running runtime')
  connection.close()
  connection = null
  await waitReady()
  assert.deepEqual(
    connection.exceptions,
    [],
    'native WebView must not double-inject the Tauri bridge',
  )
  report.checks.push('native WebView renders without uncaught bridge initialization errors')
  const screenshot = await connection.call('Page.captureScreenshot', { format: 'png' })
  await writeFile(join(output, 'android-rust-chat.png'), Buffer.from(screenshot.data, 'base64'))
  console.log(`PASS: ${report.checks.length} Android release emulator checks`)
} catch (error) {
  report.failure = error.message
  if (connection) {
    try {
      const screenshot = await connection.call('Page.captureScreenshot', { format: 'png' })
      await writeFile(
        join(output, 'android-rust-failure.png'),
        Buffer.from(screenshot.data, 'base64'),
      )
    } catch {}
  }
  throw error
} finally {
  api?.close()
  connection?.close()
  await writeFile(join(output, 'report.json'), JSON.stringify(report, null, 2) + '\n')
}
