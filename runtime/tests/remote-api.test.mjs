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
