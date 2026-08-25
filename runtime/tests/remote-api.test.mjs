// 远程访问 HTTP 层测试：状态/开关/配对码/配对/设备管理路由，
// 以及远程监听鉴权中间件的放行/拦截行为。
import assert from 'node:assert/strict'
import { mkdtempSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createApiHandler } from '../http/api-handler.mjs'
import { authorizeRemoteRequest } from '../remote-auth.mjs'
import { RemoteAccessService } from '../services/remote-access-service.mjs'

function request(method, body, headers = {}) {
  return {
    method,
    headers,
    socket: { remoteAddress: '192.168.1.20' },
    async *[Symbol.asyncIterator]() {
      if (body !== undefined) yield Buffer.from(JSON.stringify(body))
    },
  }
}

function response() {
  return {
    status: 0,
    body: '',
    destroyed: false,
    writableEnded: false,
    writeHead(status) {
      this.status = status
    },
    write(chunk = '') {
      this.body += chunk
    },
    end(chunk = '') {
      this.body += chunk
      this.writableEnded = true
    },
    on() {},
  }
}

function createRemoteFixture({ listening = true } = {}) {
  const directory = mkdtempSync(join(tmpdir(), 'pisper-remote-api-'))
  const remoteAccess = new RemoteAccessService({ dataDir: directory })
  remoteAccess.setEnabled(true)
  const remoteControl = {
    status() {
      return {
        enabled: true,
        listening,
        host: '0.0.0.0',
        port: 5174,
        tls: true,
        fingerprint: 'SHA256:ABCD',
        deviceName: '测试桌面',
        endpoints: [{ t: 'lan', url: 'https://192.168.1.5:5174' }],
        mdns: { advertising: false, error: null },
        error: null,
      }
    },
    async setEnabled() {
      return this.status()
    },
    qrPayload({ code }) {
      return {
        v: 1,
        name: '测试桌面',
        endpoints: this.status().endpoints,
        fp: 'SHA256:ABCD',
        code,
      }
    },
  }
  return { remoteAccess, remoteControl }
}

test('配对全流程：发码 → 扫码兑换 → 设备列表（带 current） → 吊销', async () => {
  const { remoteAccess, remoteControl } = createRemoteFixture()
  const handler = createApiHandler({}, { remoteAccess, remoteControl })

  // 桌面端签发配对码。
  const codeResponse = response()
  await handler(request('POST'), codeResponse, new URL('http://localhost/api/remote/pairing-code'))
  assert.equal(codeResponse.status, 200)
  const issued = JSON.parse(codeResponse.body)
  assert.match(issued.code, /^[0-9A-Z]{4}-[0-9A-Z]{4}$/)
  assert.equal(issued.qrPayload.code, issued.code)
  assert.equal(issued.qrPayload.fp, 'SHA256:ABCD')
  assert.match(issued.qrDataUrl, /^data:image\/png;base64,/)
  assert.ok(issued.qrDataUrl.length > 1_000)

  // 移动端兑换。
  const pairResponse = response()
  await handler(
    request('POST', { code: issued.code, deviceName: '测试手机' }),
    pairResponse,
    new URL('http://localhost/api/remote/pair'),
  )
  assert.equal(pairResponse.status, 201)
  const paired = JSON.parse(pairResponse.body)
  assert.ok(paired.token.startsWith('pst_'))
  assert.equal(paired.serverName, '测试桌面')
  assert.deepEqual(paired.endpoints, [{ t: 'lan', url: 'https://192.168.1.5:5174' }])
  assert.equal(paired.apiVersion, 1)

  // 设备列表：模拟远程监听鉴权后写入的当前设备标记。
  const listResponse = response()
  const listRequest = request('GET')
  listRequest.pisperDevice = { id: paired.deviceId }
  await handler(listRequest, listResponse, new URL('http://localhost/api/remote/devices'))
  const devices = JSON.parse(listResponse.body).devices
  assert.equal(devices.length, 1)
  assert.equal(devices[0].current, true)
  assert.equal(devices[0].name, '测试手机')

  // 吊销：204，且令牌立即失效。
  const revokeResponse = response()
  await handler(
    request('POST'),
    revokeResponse,
    new URL(`http://localhost/api/remote/devices/${paired.deviceId}/revoke`),
  )
  assert.equal(revokeResponse.status, 204)
  assert.equal(remoteAccess.authenticateToken(paired.token), null)
})

test('LAN 发现后的连接申请必须经桌面批准才签发令牌', async () => {
  const { remoteAccess, remoteControl } = createRemoteFixture()
  const handler = createApiHandler({}, { remoteAccess, remoteControl })

  const createResponse = response()
  const createRequest = request('POST', { deviceName: '局域网手机' })
  createRequest.pisperRemote = true
  await handler(
    createRequest,
    createResponse,
    new URL('https://desktop/api/remote/pairing-requests'),
  )
  assert.equal(createResponse.status, 202)
  const created = JSON.parse(createResponse.body)
  assert.ok(created.requestId.startsWith('pair_'))
  assert.ok(created.requestSecret.startsWith('pps_'))
  assert.equal(remoteAccess.listDevices().length, 0)

  const pendingResponse = response()
  const pendingRequest = request('GET', undefined, {
    'x-pisper-pairing-secret': created.requestSecret,
  })
  pendingRequest.pisperRemote = true
  await handler(
    pendingRequest,
    pendingResponse,
    new URL(`https://desktop/api/remote/pairing-requests/${created.requestId}`),
  )
  assert.deepEqual(JSON.parse(pendingResponse.body).status, 'pending')

  const listResponse = response()
  await handler(
    request('GET'),
    listResponse,
    new URL('http://localhost/api/remote/pairing-requests'),
  )
  assert.equal(JSON.parse(listResponse.body).requests[0].deviceName, '局域网手机')

  const invalidDecisionResponse = response()
  await handler(
    request('POST', { approved: 'true' }),
    invalidDecisionResponse,
    new URL(`http://localhost/api/remote/pairing-requests/${created.requestId}/decision`),
  )
  assert.equal(invalidDecisionResponse.status, 400)
  assert.equal(JSON.parse(invalidDecisionResponse.body).code, 'invalid_pairing_decision')

  const decisionResponse = response()
  await handler(
    request('POST', { approved: true }),
    decisionResponse,
    new URL(`http://localhost/api/remote/pairing-requests/${created.requestId}/decision`),
  )
  assert.equal(JSON.parse(decisionResponse.body).status, 'approved')

  const approvedResponse = response()
  const approvedRequest = request('GET', undefined, {
    'x-pisper-pairing-secret': created.requestSecret,
  })
  approvedRequest.pisperRemote = true
  await handler(
    approvedRequest,
    approvedResponse,
    new URL(`https://desktop/api/remote/pairing-requests/${created.requestId}`),
  )
  const approved = JSON.parse(approvedResponse.body)
  assert.equal(approved.status, 'approved')
  assert.equal(approved.serverName, '测试桌面')
  assert.ok(approved.token.startsWith('pst_'))
  assert.equal(remoteAccess.authenticateToken(approved.token)?.name, '局域网手机')
})

test('远程监听不能读取或处理其他设备的待审批申请', async () => {
  const { remoteAccess, remoteControl } = createRemoteFixture()
  const handler = createApiHandler({}, { remoteAccess, remoteControl })
  const requested = remoteAccess.requestPairingApproval({ deviceName: '手机', ip: '192.168.1.20' })
  for (const [method, path, body] of [
    ['GET', '/api/remote/pairing-requests', undefined],
    ['POST', `/api/remote/pairing-requests/${requested.requestId}/decision`, { approved: true }],
  ]) {
    const output = response()
    const input = request(method, body)
    input.pisperRemote = true
    await handler(input, output, new URL(`https://desktop${path}`))
    assert.equal(output.status, 403)
    assert.equal(JSON.parse(output.body).code, 'desktop_approval_required')
  }
})

test('配对错误映射为契约状态码', async () => {
  const { remoteAccess, remoteControl } = createRemoteFixture()
  const handler = createApiHandler({}, { remoteAccess, remoteControl })

  const invalid = response()
  await handler(
    request('POST', { code: 'WRONG-CODE' }),
    invalid,
    new URL('http://localhost/api/remote/pair'),
  )
  assert.equal(invalid.status, 403)
  assert.equal(JSON.parse(invalid.body).code, 'pairing_code_invalid')

  const missing = response()
  await handler(request('POST', {}), missing, new URL('http://localhost/api/remote/pair'))
  assert.equal(missing.status, 400)
  assert.equal(JSON.parse(missing.body).code, 'invalid_request')
})

test('远程未监听时拒绝签发配对码', async () => {
  const { remoteAccess, remoteControl } = createRemoteFixture({ listening: false })
  const handler = createApiHandler({}, { remoteAccess, remoteControl })
  const output = response()
  await handler(request('POST'), output, new URL('http://localhost/api/remote/pairing-code'))
  assert.equal(output.status, 400)
  assert.equal(JSON.parse(output.body).code, 'remote_disabled')
})

test('远程鉴权中间件：配对接口放行，其余要求有效设备令牌', () => {
  const directory = mkdtempSync(join(tmpdir(), 'pisper-remote-auth-'))
  const remoteAccess = new RemoteAccessService({ dataDir: directory })
  const { code } = remoteAccess.issuePairingCode()
  const { token, device } = remoteAccess.redeemPairingCode({ code, ip: '1' })

  // 配对接口无需凭据。
  const pairReq = request('POST', {})
  assert.equal(
    authorizeRemoteRequest(pairReq, response(), new URL('http://lan/api/remote/pair'), {
      remoteAccess,
    }),
    false,
  )

  // LAN 申请创建与凭 secret 查询同样无需已有令牌。
  assert.equal(
    authorizeRemoteRequest(
      request('POST', {}),
      response(),
      new URL('http://lan/api/remote/pairing-requests'),
      { remoteAccess },
    ),
    false,
  )
  assert.equal(
    authorizeRemoteRequest(
      request('GET'),
      response(),
      new URL('http://lan/api/remote/pairing-requests/pair_test'),
      { remoteAccess },
    ),
    false,
  )

  // 无令牌 → 401。
  const anonymousRes = response()
  assert.equal(
    authorizeRemoteRequest(request('GET'), anonymousRes, new URL('http://lan/api/sessions'), {
      remoteAccess,
    }),
    true,
  )
  assert.equal(anonymousRes.status, 401)

  // 有效令牌 → 放行，写入当前设备并跟踪响应。
  const authedReq = request('GET', undefined, { authorization: `Bearer ${token}` })
  const authedRes = response()
  assert.equal(
    authorizeRemoteRequest(authedReq, authedRes, new URL('http://lan/api/sessions'), {
      remoteAccess,
    }),
    false,
  )
  assert.equal(authedReq.pisperDevice.id, device.id)
  assert.equal(remoteAccess.activeResponses.get(device.id)?.has(authedRes), true)
})
