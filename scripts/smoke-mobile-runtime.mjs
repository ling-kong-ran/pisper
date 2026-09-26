// 在移动闭包完成裁剪后，用隔离数据目录实际发送首条消息。
// 无模型凭据时应收到会话错误；若误解析已裁掉的桌面依赖，发布构建立即失败。
import assert from 'node:assert/strict'
import { access, mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

export async function smokeMobileRuntime({ runtimeDir, runtimeProfile = 'mobile-embedded' }) {
  assert.ok(['mobile-embedded', 'mobile-store'].includes(runtimeProfile))
  const dataDir = await mkdtemp(join(tmpdir(), 'pisper-mobile-chat-smoke-'))
  const overrides = {
    PISPER_AGENT_DIR: join(dataDir, 'agent'),
    PISPER_APP_ROOT: resolve(runtimeDir),
    PISPER_DESKTOP_TOKEN: 'mobile-chat-smoke-token',
    PISPER_MOBILE_READY_FILE: join(dataDir, 'ready.json'),
    PISPER_RUNTIME_PROFILE: runtimeProfile,
    PISPER_WORKSPACE_DIR: join(dataDir, 'workspace'),
  }
  const previous = Object.fromEntries(
    [...Object.keys(overrides), 'PI_SKIP_VERSION_CHECK', 'PI_TELEMETRY', 'PI_CODING_AGENT_DIR'].map(
      (name) => [name, process.env[name]],
    ),
  )
  let app
  try {
    await mkdir(overrides.PISPER_WORKSPACE_DIR)
    await assert.rejects(
      access(join(resolve(runtimeDir), 'node_modules/@injaneity/pi-computer-use')),
      { code: 'ENOENT' },
    )
    Object.assign(process.env, overrides)
    const entry = pathToFileURL(join(resolve(runtimeDir), 'runtime/mobile-embedded.mjs')).href
    const { startEmbeddedRuntime } = await import(entry)
    app = await startEmbeddedRuntime({ startupObserver: null })
    const bootstrap = await fetch(
      `${app.url}/_pisper/desktop/bootstrap?token=mobile-chat-smoke-token`,
      { redirect: 'manual', signal: AbortSignal.timeout(30_000) },
    )
    assert.equal(bootstrap.status, 302)
    const cookie = bootstrap.headers.get('set-cookie')?.split(';')[0]
    assert.ok(cookie)
    const request = (path, body) =>
      fetch(`${app.url}${path}`, {
        method: 'POST',
        headers: { cookie, 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(30_000),
      })
    const created = await request('/api/sessions', { name: 'Mobile chat smoke' })
    assert.equal(created.status, 201)
    const session = await created.json()
    const response = await request('/api/chat', {
      sessionId: session.id,
      message: 'hello',
      attachments: [],
      goalMode: false,
      teamMode: false,
    })
    assert.equal(response.status, 200)
    const events = await response.text()
    const errorFrame = events.split('\n\n').find((frame) => frame.includes('event: error\n'))
    assert.ok(errorFrame, 'Mobile chat smoke expected a missing-model error frame')
    const dataLine = errorFrame.split('\n').find((line) => line.startsWith('data: '))
    const message = JSON.parse(dataLine.slice(6)).message
    assert.equal(typeof message, 'string')
    assert.ok(message, 'Mobile chat smoke error was empty')
    assert.doesNotMatch(message, /pi-computer-use|cannot find package|ERR_MODULE_NOT_FOUND/i)
    assert.match(message, /model|模型|provider|API key/i)
  } finally {
    try {
      await app?.close()
    } finally {
      for (const [name, value] of Object.entries(previous)) {
        if (value === undefined) delete process.env[name]
        else process.env[name] = value
      }
      await rm(dataDir, { recursive: true, force: true })
    }
  }
}
