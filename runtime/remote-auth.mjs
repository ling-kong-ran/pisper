// 远程监听的请求鉴权：配对端点公开（服务内部限流），其余请求一律要求
// 有效的设备 Bearer 令牌。与桌面 Cookie 通道互斥：远程监听不走桌面引导。
import { bearerToken } from './services/remote-access-service.mjs'

function reject(res, status, message, code) {
  const body = `${JSON.stringify({ error: message, code })}\n`
  res.writeHead(status, {
    'Cache-Control': 'no-store',
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  })
  res.end(body)
}

// 返回 true 表示请求已被拦截（401）；false 表示放行。
export function authorizeRemoteRequest(req, res, url, { remoteAccess }) {
  // 配对码兑换，以及 LAN 申请的创建/凭 secret 查询无需已有设备令牌。
  // 审批列表与决定接口不在此白名单内，并由路由再次限制为桌面监听来源。
  const pairingRequestStatus =
    (req.method === 'GET' || req.method === 'DELETE') &&
    /^\/api\/remote\/pairing-requests\/[^/]+$/.test(url.pathname)
  if (
    url.pathname === '/api/remote/pair' ||
    (req.method === 'POST' && url.pathname === '/api/remote/pairing-requests') ||
    pairingRequestStatus
  ) {
    return false
  }
  const device = remoteAccess.authenticateToken(bearerToken(req))
  if (!device) {
    reject(res, 401, 'Remote access authentication is required.', 'remote_auth_required')
    return true
  }
  // 供路由识别"当前设备"（如设备列表的 current 标记）。
  req.pisperDevice = device
  // 跟踪活跃响应：吊销设备时主动断开其长连接。
  remoteAccess.trackResponse(device.id, res)
  return false
}
