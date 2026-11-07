import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { BrowserAutomationService } from '../services/browser-automation-service.mjs'
import { DesktopBrowserHostService } from '../services/desktop-browser-host-service.mjs'

function connectedHost(options) {
  const host = new DesktopBrowserHostService(options)
  const sent = []
  const detach = host.attach((event, data) => sent.push({ event, data }))
  return {
    host,
    sent,
    detach,
    lastCommand: () => sent.filter((s) => s.event === 'command').at(-1).data,
  }
}

test('commands go to the connected desktop front end and results come back trimmed', async () => {
  const { host, lastCommand } = connectedHost()
  const opening = host.execute('session-1', { action: 'open', url: 'https://example.com/' })
  const command = lastCommand()
  assert.equal(command.sessionId, 'session-1')
  assert.equal(command.action, 'open')
  assert.equal(command.url, 'https://example.com/')
  host.settle(command.requestId, {
    ok: true,
    result: { url: 'https://example.com/', title: 'x'.repeat(900), secret: 'dropped' },
  })
  const result = await opening
  assert.equal(result.action, 'open')
  assert.equal(result.title.length, 500)
  assert.equal('secret' in result, false)

  // 网页给的内容不可信：字段类型、数量和长度都收口。
  const inspecting = host.execute('session-1', { action: 'inspect' })
  host.settle(lastCommand().requestId, {
    ok: true,
    result: {
      url: 42,
      text: 'y'.repeat(30_000),
      elements: [...Array(150)].map((_, index) => ({ selector: `#a${index}`, disabled: 'yes' })),
    },
  })
  const snapshot = await inspecting
  assert.equal(snapshot.url, '')
  assert.equal(snapshot.text.length, 20_000)
  assert.equal(snapshot.elements.length, 100)
  assert.equal(snapshot.elements[0].disabled, false)
})

test('a failed, unknown or late result cannot confuse a request', async () => {
  const { host, lastCommand } = connectedHost()
  const clicking = host.execute('s', { action: 'click', selector: '#go' })
  assert.equal(host.settle('someone-else', { ok: true, result: {} }), false)
  const { requestId } = lastCommand()
  assert.equal(host.settle(requestId, { ok: false, error: 'No visible element matches #go' }), true)
  await assert.rejects(clicking, /No visible element matches #go/)
  assert.equal(host.settle(requestId, { ok: true, result: {} }), false)
})

test('disconnecting fails what is pending, and a newer connection replaces the old one', async () => {
  const { host, detach } = connectedHost()
  const pending = host.execute('s', { action: 'wait' })
  const newerSent = []
  const detachNewer = host.attach((event, data) => newerSent.push({ event, data }))
  await assert.rejects(pending, /reconnected/)
  // 旧连接晚到的断开不能把新连接踢掉。
  detach()
  assert.equal(host.available, true)
  const next = host.execute('s', { action: 'wait' })
  assert.equal(newerSent.length, 1)
  detachNewer()
  assert.equal(host.available, false)
  await assert.rejects(next, /disconnected/)
})

test('a stuck front end times out and stopping the run cancels the action', async () => {
  const timers = []
  const { host, sent } = connectedHost({
    setTimer: (callback) => (timers.push(callback), timers.length),
    clearTimer: () => {},
  })
  const stuck = host.execute('s', { action: 'inspect' })
  timers[0]()
  await assert.rejects(stuck, /timed out/)

  const controller = new AbortController()
  const running = host.execute('s', { action: 'wait' }, { signal: controller.signal })
  controller.abort(new Error('stopped by user'))
  await assert.rejects(running, /stopped by user/)
  assert.deepEqual(sent.at(-1), { event: 'cancel', data: { sessionId: 's' } })
})

test('a screenshot is written to the path the service chose, only if it really is a PNG', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-host-'))
  try {
    const { host, lastCommand } = connectedHost()
    const outputPath = join(directory, 'shot.png')
    const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 1, 2, 3])
    const capturing = host.execute('s', { action: 'screenshot', outputPath })
    // 路径不发给前端，前端也改不了它。
    assert.equal('outputPath' in lastCommand(), false)
    host.settle(lastCommand().requestId, {
      ok: true,
      result: { imageBase64: png.toString('base64'), url: 'https://a.example/', path: '/etc/evil' },
    })
    const result = await capturing
    assert.equal(result.path, outputPath)
    assert.equal(result.fullPage, false)
    assert.deepEqual(await readFile(outputPath), png)

    const rejected = host.execute('s', { action: 'screenshot', outputPath })
    host.settle(lastCommand().requestId, {
      ok: true,
      result: { imageBase64: Buffer.from('<html>').toString('base64') },
    })
    await assert.rejects(rejected, /PNG/)
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

test('browser automation prefers the visible browser and only falls back when nobody is connected', async () => {
  const host = new DesktopBrowserHostService()
  const service = new BrowserAutomationService({ desktopHost: host, idleTimeoutMs: 0 })
  const launched = []
  service.launchPlaywright = async () => {
    launched.push(true)
    return {
      page: {
        setViewportSize: async () => {},
        goto: async () => {},
        url: () => 'https://headless.example/',
        title: async () => 'headless',
      },
    }
  }
  // 没连：无头浏览器。这个会话之后也不中途换，模型手里的页面状态在那边。
  const headless = await service.execute('old', { action: 'open', url: 'https://a.example/' })
  assert.equal(headless.title, 'headless')

  const sent = []
  host.attach((event, data) => sent.push({ event, data }))
  await service.execute('old', { action: 'open', url: 'https://a.example/' })
  assert.equal(sent.length, 0)
  assert.equal(launched.length, 1)

  // 连着：新会话走右侧浏览器，同样校验地址，不开无头浏览器。
  await assert.rejects(
    service.execute('new', { action: 'open', url: 'file:///etc/passwd' }),
    /http and https/,
  )
  const opening = service.execute('new', { action: 'open', url: 'https://b.example/' })
  await new Promise((resolve) => setImmediate(resolve))
  const command = sent.at(-1).data
  assert.equal(command.url, 'https://b.example/')
  host.settle(command.requestId, { ok: true, result: { url: command.url, title: 'visible' } })
  assert.equal((await opening).title, 'visible')
  assert.equal(launched.length, 1)

  // 空闲超时把页面还给用户。
  await service.closeSession('new')
  assert.deepEqual(sent.at(-1), { event: 'release', data: { sessionId: 'new' } })
})

test('the host route streams commands over real HTTP and takes results back', async () => {
  const { createServer } = await import('node:http')
  const { createApiHandler } = await import('../http/api-handler.mjs')
  const host = new DesktopBrowserHostService()
  const handler = createApiHandler({ desktopBrowserHost: host })
  const server = createServer(
    (req, res) => void handler(req, res, new URL(req.url, 'http://127.0.0.1')),
  )
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const base = `http://127.0.0.1:${server.address().port}`
  const controller = new AbortController()
  try {
    const response = await fetch(`${base}/api/desktop-browser/host`, { signal: controller.signal })
    assert.equal(response.status, 200)
    const reader = response.body.getReader()
    let buffer = ''
    const readUntil = async (pattern) => {
      while (!pattern.test(buffer)) {
        const { value, done } = await reader.read()
        if (done) throw new Error(`stream ended before ${pattern}`)
        buffer += Buffer.from(value).toString('utf8')
      }
    }
    await readUntil(/event: ready/)
    assert.equal(host.available, true)
    const opening = host.execute('s', { action: 'open', url: 'https://a.example/' })
    await readUntil(/event: command[\s\S]*\n\n/)
    const { requestId } = JSON.parse(buffer.match(/event: command\n(?:id: .*\n)?data: (.*)/)[1])
    const posted = await fetch(`${base}/api/desktop-browser/host/${requestId}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ ok: true, result: { url: 'https://a.example/', title: 'A' } }),
    })
    assert.deepEqual(await posted.json(), { accepted: true })
    assert.equal((await opening).title, 'A')
    // 断开后 Runtime 知道宿主不在了，退回无头浏览器。
    controller.abort()
    for (let waited = 0; host.available && waited < 2_000; waited += 20)
      await new Promise((resolve) => setTimeout(resolve, 20))
    assert.equal(host.available, false)
  } finally {
    controller.abort()
    await new Promise((resolve) => server.close(resolve))
  }
})

test('the host route is closed to remote clients and the host only connects where it can work', async () => {
  const { desktopRoutes } = await import('../http/routes/desktop.mjs')
  for (const route of desktopRoutes.filter((entry) =>
    entry.path.startsWith('/api/desktop-browser/'),
  )) {
    let status = 0
    await route.handler({
      req: { pisperRemote: true },
      json: (code) => {
        status = code
      },
    })
    assert.equal(status, 403, route.path)
  }
  const host = await readFile('src/features/chat/agent-browser-host.ts', 'utf8')
  assert.match(host, /platform !== 'darwin' && platform !== 'win32'/)
  // 指针和光效要进得了 CSP 严格的页面：不用 <style>，也不用 innerHTML。
  const scripts = await readFile('src/features/chat/agent-browser-scripts.ts', 'utf8')
  assert.doesNotMatch(scripts, /\.innerHTML|createElement\('style'\)|setAttribute\('style'/)
})

test('computer use goes to the desktop app, returns a screenshot and hands the screen back', async () => {
  const { DesktopComputerService } = await import('../services/desktop-computer-service.mjs')
  const service = new DesktopComputerService()
  await assert.rejects(service.execute('s', { action: 'screenshot' }), /desktop app/)
  const sent = []
  service.attach((event, data) => sent.push({ event, data }))
  const clicking = service.execute('s', { action: 'click', x: 10, y: 20 })
  const command = sent.at(-1).data
  assert.deepEqual(
    { action: command.action, x: command.x, y: command.y, sessionId: command.sessionId },
    { action: 'click', x: 10, y: 20, sessionId: 's' },
  )
  service.settle(command.requestId, {
    ok: true,
    result: { imageBase64: 'abc', width: 1440, height: 900, extra: 'dropped' },
  })
  assert.deepEqual(await clicking, {
    action: 'click',
    imageBase64: 'abc',
    width: 1440,
    height: 900,
  })
  // 动作成功了但没拿到截图，模型就是在盲操作：算失败。
  const blind = service.execute('s', { action: 'screenshot' })
  service.settle(sent.at(-1).data.requestId, { ok: true, result: {} })
  await assert.rejects(blind, /usable screenshot/)
  // 被用户按 Esc 停掉时，原生层的说明要传给模型。
  const stopped = service.execute('s', { action: 'type', text: 'x' })
  service.settle(sent.at(-1).data.requestId, { ok: false, error: 'The user stopped computer use.' })
  await assert.rejects(stopped, /user stopped/)
  service.release('s')
  assert.deepEqual(sent.at(-1), { event: 'release', data: { sessionId: 's' } })
})

test('computer use asks once per session, even in auto mode, and hands the screen back to ask', async () => {
  const { SessionPermissionService } = await import('../services/session-permission-service.mjs')
  const requested = []
  const released = []
  const service = new SessionPermissionService({
    getMode: () => 'auto',
    getExecutionMode: () => 'auto',
    getToolRisk: () => 'high',
    isThirdPartyTool: () => false,
    isToolBlocked: () => false,
    onApprovalRequested: (sessionId) => released.push(sessionId),
    emit: () => {},
  })
  service.requestApproval = (input) => {
    requested.push(input.toolName)
    service.onApprovalRequested?.(input.sessionId)
    return Promise.resolve({ approved: true })
  }
  const call = (sessionId, args) =>
    service.authorize({
      sessionId,
      cwd: '/tmp',
      toolName: 'computer_use',
      toolCallId: 't',
      args,
    })
  assert.equal(await call('a', { action: 'click', x: 1, y: 1 }), undefined)
  assert.equal(await call('a', { action: 'type', text: 'x' }), undefined)
  assert.deepEqual(requested, ['computer_use'])
  assert.deepEqual(released, ['a'])
  // 换一个会话要重新问。
  await call('b', { action: 'screenshot' })
  assert.deepEqual(requested, ['computer_use', 'computer_use'])
})

test('computer use stays with the primary agent and bot conversations, and is desktop only', async () => {
  const [multiAgent, capabilities, browserHost, bridge] = await Promise.all([
    readFile('runtime/services/multi-agent-service.mjs', 'utf8'),
    readFile('runtime/runtime-capabilities.mjs', 'utf8'),
    readFile('src-tauri/src/desktop_shell/computer_control.rs', 'utf8'),
    readFile('src-tauri/capabilities/desktop-computer.json', 'utf8'),
  ])
  assert.match(multiAgent, /PARENT_ONLY_TOOL_NAMES = new Set\(\[[\s\S]*?'computer_use'/)
  assert.match(capabilities, /if \(features\.computerUse\) tools\.add\('computer_use'\)/)
  assert.deepEqual(JSON.parse(bridge).windows, ['main'])
  // 原生层拒绝操作 Pisper 自己：不能靠模型遵守提示词，否则它能点自己的“批准”。
  assert.match(browserHost, /point_hits_own_window\(x, y\)/)
  assert.match(browserHost, /foreground_is_own\(\)/)
  // Agent 自己发的 Esc 不能把自己停掉。
  assert.match(browserHost, /LLKHF_INJECTED/)
  // 遮罩不能挡输入，也不能抢焦点。
  assert.match(browserHost, /set_ignore_cursor_events\(true\)/)
  assert.match(browserHost, /focusable\(false\)/)
})
