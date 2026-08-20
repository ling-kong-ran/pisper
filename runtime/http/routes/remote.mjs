// 远程访问路由：开关、状态、配对码签发、配对兑换、设备管理。
// 这些路由在回环监听与远程监听上都可达：回环侧供桌面 UI 管理用，
// 远程侧供已配对设备（Bearer）查看/管理自身。
import { RemoteAccessError } from '../../services/remote-access-service.mjs'

function remoteErrorStatus(code) {
  switch (code) {
    case 'pairing_code_invalid':
      return 403
    case 'pairing_code_expired':
      return 410
    case 'pairing_rate_limited':
      return 429
    case 'device_not_found':
      return 404
    default:
      return 400
  }
}

// RemoteAccessError 映射为契约约定的状态码与机读 code；其他错误继续上抛。
function respondRemoteError(json, error) {
  if (error instanceof RemoteAccessError) {
    json(remoteErrorStatus(error.code), { error: error.message, code: error.code })
    return
  }
  throw error
}

export const remoteRoutes = [
  {
    method: 'GET',
    path: '/api/remote/status',
    handler({ services, json }) {
      json(200, { apiVersion: 1, ...services.remoteControl.status() })
    },
  },
  {
    method: 'PUT',
    path: '/api/remote/enabled',
    async handler({ services, body, json }) {
      const input = await body()
      await services.remoteControl.setEnabled(Boolean(input.enabled))
      json(200, services.remoteControl.status())
    },
  },
  {
    method: 'POST',
    path: '/api/remote/pairing-code',
    async handler({ services, json }) {
      const status = services.remoteControl.status()
      if (!status.listening) {
        json(400, { error: '远程访问未启用，请先开启。', code: 'remote_disabled' })
        return
      }
      const { code, expiresAt } = services.remoteAccess.issuePairingCode()
      const qrPayload = services.remoteControl.qrPayload({ code })
      // 二维码在服务端渲染为 data URL：runtime 已有 qrcode 依赖，
      // 前端/其他客户端无需额外依赖即可展示。
      let qrDataUrl = ''
      try {
        const QRCode = await import('qrcode')
        qrDataUrl = await QRCode.toDataURL(JSON.stringify(qrPayload), { margin: 1, width: 320 })
      } catch {
        // 渲染失败不阻断配对：客户端仍可手动输入配对码。
      }
      json(200, { code, expiresAt, qrPayload, qrDataUrl })
    },
  },
  {
    method: 'POST',
    path: '/api/remote/pair',
    async handler({ services, body, req, json }) {
      try {
        const input = await body()
        const code = String(input.code || '')
        if (!code) {
          json(400, { error: '缺少配对码。', code: 'invalid_request' })
          return
        }
        const result = services.remoteAccess.redeemPairingCode({
          code,
          deviceName: input.deviceName,
          ip: req.socket?.remoteAddress || 'unknown',
        })
        json(201, {
          deviceId: result.device.id,
          token: result.token,
          serverName: services.remoteControl.status().deviceName,
          apiVersion: 1,
        })
      } catch (error) {
        respondRemoteError(json, error)
      }
    },
  },
  {
    method: 'GET',
    path: '/api/remote/devices',
    handler({ services, req, json }) {
      json(200, { devices: services.remoteAccess.listDevices(req.pisperDevice?.id || null) })
    },
  },
  {
    method: 'POST',
    path: '/api/remote/devices/:deviceId/revoke',
    handler({ services, params, res, json }) {
      try {
        services.remoteAccess.revokeDevice(params.deviceId, { except: res })
        res.writeHead(204)
        res.end()
      } catch (error) {
        respondRemoteError(json, error)
      }
    },
  },
]
