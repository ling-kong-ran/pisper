import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { BrowserAutomationService } from '../services/browser-automation-service.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'
import { createBrowserAutomationTool } from '../tools/app/browser-automation.mjs'

test('browser automation delegates bounded actions and archives screenshots', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-browser-tool-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const calls = []
  const generated = []
  const driver = {
    async execute(sessionId, input) {
      calls.push({ sessionId, input })
      if (input.action === 'screenshot')
        return {
          action: 'screenshot',
          path: input.outputPath,
          name: 'page.png',
          mimeType: 'image/png',
        }
      return { action: input.action, url: input.url || 'https://example.com/' }
    },
  }
  const service = new BrowserAutomationService({ driver })
  const tool = createBrowserAutomationTool({
    cwd: directory,
    browserSessionId: 'session-1',
    browserAutomationService: service,
    onGeneratedFile: (result) => generated.push(result),
  })

  await tool.execute('open-1', { action: 'open', url: 'https://example.com' })
  const screenshot = await tool.execute('shot-1', {
    action: 'screenshot',
    outputName: '../unsafe name.png',
  })

  assert.equal(calls[0].sessionId, 'session-1')
  assert.equal(
    calls[1].input.outputPath,
    join(directory, 'generated', 'browser', 'unsafe-name.png'),
  )
  assert.deepEqual(generated, [screenshot.details])
})

test('browser automation releases an idle desktop browser session', async () => {
  const closed = []
  const callbacks = []
  const service = new BrowserAutomationService({
    driver: {
      async execute(_sessionId, input) {
        return { action: input.action, url: input.url }
      },
      async closeSession(sessionId) {
        closed.push(sessionId)
        return true
      },
    },
    idleTimeoutMs: 1,
    setTimer(callback) {
      callbacks.push(callback)
      return { unref() {} }
    },
    clearTimer() {},
  })

  await service.execute('session-idle', { action: 'open', url: 'https://example.com' })
  callbacks.at(-1)()
  await new Promise((resolveWait) => setImmediate(resolveWait))

  assert.deepEqual(closed, ['session-idle'])
})

test('full-access browser actions run without approval', () => {
  const base = {
    mode: 'auto',
    executionMode: 'full-access',
    cwd: '/workspace',
    toolName: 'browser_automation',
    toolRisk: 'medium',
  }

  assert.equal(permissionRequirement({ ...base, args: { action: 'inspect' } }), null)
  assert.equal(permissionRequirement({ ...base, args: { action: 'screenshot' } }), null)
  assert.equal(permissionRequirement({ ...base, args: { action: 'click' } }), null)
  assert.equal(permissionRequirement({ ...base, args: { action: 'type' } }), null)
})
