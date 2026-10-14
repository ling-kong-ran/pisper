import assert from 'node:assert/strict'
import test from 'node:test'
import { MobileOperationService } from '../services/mobile-operation-service.mjs'

test('移动设备操作只向当前会话通道发送并等待对应结果', async () => {
  const service = new MobileOperationService({ timeoutMs: 1_000 })
  const events = []
  const detach = service.attach('session-a', (event, data) => events.push({ event, data }))
  const pending = service.execute('session-a', 'contacts.search', { query: 'Ada', limit: 5 })

  assert.equal(events.length, 1)
  assert.equal(events[0].event, 'mobile_operation_request')
  assert.equal(events[0].data.sessionId, 'session-a')
  assert.equal(events[0].data.operation, 'contacts.search')
  assert.match(events[0].data.id, /^mop_[a-f0-9]{32}$/)
  assert.equal(service.resolve('session-b', events[0].data.id, { ok: true, result: {} }), false)
  assert.equal(
    service.resolve('session-a', events[0].data.id, {
      ok: true,
      result: { contacts: [{ name: 'Ada', phones: ['10086'] }] },
    }),
    true,
  )
  assert.deepEqual(await pending, { contacts: [{ name: 'Ada', phones: ['10086'] }] })
  detach()
  service.dispose()
})

test('移动端拒绝、断连与结果大小限制不会泄漏挂起操作', async () => {
  const service = new MobileOperationService({ timeoutMs: 1_000 })
  const events = []
  service.attach('session-a', (event, data) => events.push({ event, data }))

  const controller = new AbortController()
  const aborted = service.execute('session-a', 'contacts.search', {}, { signal: controller.signal })
  controller.abort()
  await assert.rejects(aborted, /已取消/)
  assert.equal(events.at(-1).event, 'mobile_operation_cancel')
  assert.equal(events.at(-1).data.reason, 'aborted')

  const rejected = service.execute('session-a', 'location.current')
  service.resolve('session-a', events.at(-1).data.id, { ok: false, error: 'permission denied' })
  await assert.rejects(rejected, /permission denied/)

  const oversized = service.execute('session-a', 'camera.capture')
  service.resolve('session-a', events.at(-1).data.id, {
    ok: true,
    result: { data: 'x'.repeat(12 * 1024 * 1024) },
  })
  await assert.rejects(oversized, /12 MB/)

  service.dispose()
  await assert.rejects(service.execute('session-b', 'contacts.search'), /没有已连接/)
})
