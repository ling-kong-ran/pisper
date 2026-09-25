import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  withActionVerification,
  withComputerUseVerification,
} from '../runtime/computer-use-verification.mjs'
import { getOfficialComputerUseExtensionPath } from '../runtime/computer-use-extension.mjs'

test('动作包装保留参数与调用上下文，关闭时不请求验证，失败时保留动作结果', async () => {
  let enabled = false
  let verificationCalls = 0
  const result = { content: [{ type: 'text', text: 'after action' }], details: { stateId: 'next' } }
  const ctx = {}
  const signal = new AbortController().signal
  const update = () => undefined
  const tool = {
    parameters: {
      type: 'object',
      properties: { stateId: { type: 'string' } },
      required: ['stateId'],
    },
    execute: async (id, params, receivedSignal, onUpdate, receivedCtx) => {
      assert.equal(id, 'call')
      assert.deepEqual(params, { stateId: 'current' })
      assert.equal(receivedSignal, signal)
      assert.equal(onUpdate, update)
      assert.equal(receivedCtx, ctx)
      return result
    },
  }
  const wrapped = withActionVerification(tool, {
    actionVerificationEnabled: () => enabled,
    verifyActionOutcome: async (input, options) => {
      verificationCalls += 1
      assert.deepEqual(input, { expectation: 'done', outcomeText: 'after action' })
      assert.equal(options.signal, signal)
      throw Object.assign(new Error('test'), { code: 'network' })
    },
  })
  assert.deepEqual(wrapped.parameters.required, tool.parameters.required)
  assert.equal(tool.parameters.properties.verify, undefined)
  const execute = () =>
    wrapped.execute('call', { stateId: 'current', verify: 'done' }, signal, update, ctx)
  assert.equal(await execute(), result)
  assert.equal(verificationCalls, 0)
  enabled = true
  assert.equal((await execute()).details.actionVerification.status, 'unavailable')
  assert.equal(result.content.length, 1)
  assert.equal(verificationCalls, 1)
})

test('Pi 加载的 act_ui 包装与官方会话恢复共享 stateId，重载与停止保留原生命周期', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-cu-verify-'))
  const previous = process.env.PI_CODING_AGENT_DIR
  process.env.PI_CODING_AGENT_DIR = directory
  t.after(async () => {
    if (previous === undefined) delete process.env.PI_CODING_AGENT_DIR
    else process.env.PI_CODING_AGENT_DIR = previous
    await rm(directory, { recursive: true, force: true })
  })
  const entry = import.meta.resolve('@earendil-works/pi-coding-agent')
  const { loadExtensions } = await import(new URL('./core/extensions/loader.js', entry).href)
  const loaded = await loadExtensions([getOfficialComputerUseExtensionPath()], directory)
  assert.deepEqual(loaded.errors, [])
  const original = loaded.extensions[0]
  const ctx = {
    cwd: directory,
    hasUI: false,
    sessionManager: {
      getBranch: () => [
        {
          type: 'message',
          message: {
            role: 'toolResult',
            toolName: 'observe_ui',
            details: {
              target: { app: 'test', pid: 99999999, windowId: 123 },
              capture: {
                stateId: 'restored-state',
                width: 1,
                height: 1,
                scaleFactor: 1,
                timestamp: 1,
              },
              outline: { root: { ref: '@e1', children: [] }, lookId: 'test-look' },
            },
          },
        },
      ],
    },
  }
  t.after(async () => {
    for (const handler of original.handlers.get('session_shutdown')) await handler({}, ctx)
  })
  const adapted = withComputerUseVerification(loaded, { actionVerificationEnabled: () => false })
  const wrapped = adapted.extensions[0]
  assert.equal(wrapped.handlers, original.handlers)
  assert.equal(wrapped.tools.size, original.tools.size)
  assert.equal(original.tools.get('act_ui').definition.parameters.properties.verify, undefined)
  for (const handler of wrapped.handlers.get('session_start')) await handler({}, ctx)
  const act = () =>
    wrapped.tools
      .get('act_ui')
      .definition.execute(
        'test',
        { stateId: 'restored-state', actions: [] },
        AbortSignal.abort(),
        undefined,
        ctx,
      )
  // 已取消的调用在任何原生访问前退出；若状态不共享，会先报 State unavailable。
  await assert.rejects(act(), /Operation aborted/)
  for (const handler of wrapped.handlers.get('session_start')) await handler({}, ctx)
  await assert.rejects(act(), /Operation aborted/)
  for (const handler of wrapped.handlers.get('session_shutdown')) await handler({}, ctx)
  await assert.rejects(act(), /State .* unavailable/)
})
