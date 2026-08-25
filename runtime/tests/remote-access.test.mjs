// 远程访问服务的单元测试：配对码生命周期、限流、设备令牌、吊销与持久化。
import assert from 'node:assert/strict'
import { mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  formatPairingCode,
  isPrivateNetworkAddress,
  normalizePairingCode,
  RemoteAccessError,
  RemoteAccessService,
} from '../services/remote-access-service.mjs'

function createService() {
  const directory = mkdtempSync(join(tmpdir(), 'pisper-remote-'))
  return new RemoteAccessService({ dataDir: directory })
}

test('配对码一次性：兑换成功后旧码失效', () => {
  const service = createService()
  const { code } = service.issuePairingCode()
  const first = service.redeemPairingCode({ code, deviceName: '手机 A', ip: '10.0.0.2' })
  assert.ok(first.token.startsWith('pst_'))
  assert.equal(first.device.name, '手机 A')
  assert.throws(
    () => service.redeemPairingCode({ code, deviceName: '手机 B', ip: '10.0.0.3' }),
    (error) => error instanceof RemoteAccessError && error.code === 'pairing_code_invalid',
  )
})

test('新配对码作废旧码', () => {
  const service = createService()
  const first = service.issuePairingCode()
  const second = service.issuePairingCode()
  assert.notEqual(first.code, second.code)
  assert.throws(
    () => service.redeemPairingCode({ code: first.code, ip: '10.0.0.2' }),
    (error) => error.code === 'pairing_code_invalid',
  )
  assert.ok(service.redeemPairingCode({ code: second.code, ip: '10.0.0.2' }).token)
})

test('配对码归一化：忽略大小写与分隔符', () => {
  assert.equal(normalizePairingCode('abcd-efgh'), 'ABCDEFGH')
  assert.equal(formatPairingCode('abcdefgh'), 'ABCD-EFGH')
  const service = createService()
  const { code } = service.issuePairingCode()
  // 用小写 + 无连字符形式也能兑换。
  assert.ok(service.redeemPairingCode({ code: code.toLowerCase().replace('-', ''), ip: '1' }))
})

test('配对码过期后拒绝兑换', () => {
  const service = createService()
  const { code } = service.issuePairingCode()
  service.state.pairingCode.expiresAt = new Date(Date.now() - 1000).toISOString()
  assert.throws(
    () => service.redeemPairingCode({ code, ip: '1' }),
    (error) => error.code === 'pairing_code_expired',
  )
})

test('连续失败后限流，窗口过后恢复', () => {
  const service = createService()
  service.issuePairingCode()
  for (let index = 0; index < 5; index += 1) {
    assert.throws(
      () => service.redeemPairingCode({ code: 'WRONGCODE', ip: '10.0.0.9' }),
      (error) => error.code === 'pairing_code_invalid',
    )
  }
  // 第 6 次：即使配对码正确也被限流。
  assert.throws(
    () => service.redeemPairingCode({ code: 'WRONGCODE', ip: '10.0.0.9' }),
    (error) => error.code === 'pairing_rate_limited',
  )
  // 手动推进冷却窗口。
  service.failures.get('10.0.0.9').resetAt = Date.now() - 1
  assert.throws(
    () => service.redeemPairingCode({ code: 'WRONGCODE', ip: '10.0.0.9' }),
    (error) => error.code === 'pairing_code_invalid',
  )
})

test('LAN 配对申请只接受私网并由桌面批准后一次性领取令牌', () => {
  assert.equal(isPrivateNetworkAddress('192.168.1.20'), true)
  assert.equal(isPrivateNetworkAddress('::ffff:10.0.0.2'), true)
  assert.equal(isPrivateNetworkAddress('fe80::1234%wlan0'), true)
  assert.equal(isPrivateNetworkAddress('8.8.8.8'), false)
  assert.equal(isPrivateNetworkAddress('fc-not-an-ip'), false)

  const service = createService()
  assert.throws(
    () => service.requestPairingApproval({ deviceName: '公网设备', ip: '8.8.8.8' }),
    (error) => error.code === 'pairing_lan_required',
  )
  const requested = service.requestPairingApproval({ deviceName: '手机 A', ip: '192.168.1.20' })
  assert.equal(service.listPairingApprovals()[0].deviceName, '手机 A')
  assert.throws(
    () => service.pairingApprovalStatus(requested.requestId, 'wrong-secret'),
    (error) => error.code === 'pairing_request_not_found',
  )
  assert.equal(
    service.pairingApprovalStatus(requested.requestId, requested.secret).status,
    'pending',
  )
  assert.equal(service.resolvePairingApproval(requested.requestId, true).status, 'approved')
  const approved = service.pairingApprovalStatus(requested.requestId, requested.secret)
  assert.equal(approved.status, 'approved')
  assert.ok(approved.token.startsWith('pst_'))
  assert.equal(service.authenticateToken(approved.token)?.name, '手机 A')
  assert.throws(
    () => service.pairingApprovalStatus(requested.requestId, requested.secret),
    (error) => error.code === 'pairing_request_not_found',
  )
})

test('拒绝 LAN 配对申请不会创建授权设备', () => {
  const service = createService()
  const requested = service.requestPairingApproval({ deviceName: '手机 B', ip: '172.16.0.9' })
  service.resolvePairingApproval(requested.requestId, false)
  assert.deepEqual(service.pairingApprovalStatus(requested.requestId, requested.secret), {
    status: 'rejected',
  })
  assert.equal(service.listDevices().length, 0)
})

test('已批准但未领取的 LAN 配对结果过期后吊销授权设备', () => {
  const service = createService()
  const requested = service.requestPairingApproval({ deviceName: '手机 C', ip: '10.0.0.8' })
  service.resolvePairingApproval(requested.requestId, true)
  const pending = service.pendingPairingRequests.get(requested.requestId)
  const token = pending.result.token
  const cleanupAt = Date.parse(pending.resolvedAt) + 60_001

  assert.equal(service.authenticateToken(token)?.name, '手机 C')
  service.cleanupPairingRequests(cleanupAt)
  assert.equal(service.authenticateToken(token), null)
  assert.equal(service.listDevices()[0].revokedAt !== null, true)
  assert.equal(service.pendingPairingRequests.has(requested.requestId), false)
})

test('批准结果不受原申请期限截断，关闭远程访问会吊销未领取授权', () => {
  const service = createService()
  service.setEnabled(true)
  const requested = service.requestPairingApproval({ deviceName: '手机 D', ip: '10.0.0.9' })
  service.resolvePairingApproval(requested.requestId, true)
  service.pendingPairingRequests.get(requested.requestId).expiresAt = new Date(
    Date.now() - 1,
  ).toISOString()

  const approved = service.pairingApprovalStatus(requested.requestId, requested.secret)
  assert.equal(approved.status, 'approved')
  assert.equal(service.authenticateToken(approved.token)?.name, '手机 D')

  const unclaimed = service.requestPairingApproval({ deviceName: '手机 E', ip: '10.0.0.10' })
  service.resolvePairingApproval(unclaimed.requestId, true)
  const token = service.pendingPairingRequests.get(unclaimed.requestId).result.token
  service.setEnabled(false)
  assert.equal(service.authenticateToken(token), null)
  assert.equal(service.pendingPairingRequests.size, 0)
})

test('设备令牌认证与吊销：吊销后 401 且活跃连接被断开', () => {
  const service = createService()
  const { code } = service.issuePairingCode()
  const { device, token } = service.redeemPairingCode({ code, deviceName: '手机', ip: '1' })

  assert.equal(service.authenticateToken(token)?.id, device.id)
  assert.equal(service.authenticateToken('pst_wrong'), null)
  assert.equal(service.authenticateToken(''), null)

  // 模拟该设备持有一个 SSE 响应。
  let ended = false
  const handlers = new Map()
  const res = {
    end() {
      ended = true
    },
    on(event, callback) {
      handlers.set(event, callback)
    },
  }
  service.trackResponse(device.id, res)
  service.revokeDevice(device.id)
  assert.equal(ended, true)
  assert.equal(service.authenticateToken(token), null)
  assert.throws(
    () => service.revokeDevice(device.id),
    (error) => error.code === 'device_not_found',
  )
})

test('状态持久化：重新加载后设备与开关保留，令牌哈希不落明文', () => {
  const directory = mkdtempSync(join(tmpdir(), 'pisper-remote-'))
  const service = new RemoteAccessService({ dataDir: directory })
  service.setEnabled(true)
  const { code } = service.issuePairingCode()
  const { token } = service.redeemPairingCode({ code, deviceName: '平板', ip: '1' })

  const reloaded = new RemoteAccessService({ dataDir: directory })
  assert.equal(reloaded.isEnabled(), true)
  assert.equal(reloaded.listDevices().length, 1)
  assert.equal(reloaded.authenticateToken(token)?.name, '平板')

  const raw = JSON.parse(readFileSync(join(directory, 'remote-access.json'), 'utf8'))
  assert.equal(JSON.stringify(raw).includes(token), false)
})
