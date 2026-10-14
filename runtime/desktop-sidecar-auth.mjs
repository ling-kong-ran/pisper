// 桌面 sidecar 的请求鉴权：Tauri WebView 先访问引导路径换取 HttpOnly Cookie，
// 之后的 API 请求凭 Cookie 通过；同时校验非幂等请求的 Origin 防止跨站伪造。
import { timingSafeEqual } from 'node:crypto'

export const DESKTOP_BOOTSTRAP_PATH = '/_pisper/desktop/bootstrap'
export const DESKTOP_COOKIE_NAME = '__pisper_desktop'

// 常量时间比较：令牌等敏感串不能用 === 直接比较，避免时序侧信道攻击。
function secureEqual(left, right) {
  const leftBytes = Buffer.from(String(left || ''))
  const rightBytes = Buffer.from(String(right || ''))
  return leftBytes.length === rightBytes.length && timingSafeEqual(leftBytes, rightBytes)
}

// 从 Cookie 头解析指定名称的 Cookie 值（简易解析，不依赖第三方库）。
function cookieValue(header, name) {
  for (const entry of String(header || '').split(';')) {
    const separator = entry.indexOf('=')
    if (separator < 0) continue
    if (entry.slice(0, separator).trim() === name) return entry.slice(separator + 1).trim()
  }
  return ''
}

// 引导跳转地址白名单化：只允许站内路径，拒绝 //host 跳转与反斜杠等异常输入，防止开放重定向。
function bootstrapLocation(value) {
  const location = String(value || '')
  if (!location.startsWith('/') || location.startsWith('//') || location.includes('\\')) return '/'
  try {
    const parsed = new URL(location, 'http://127.0.0.1')
    return parsed.origin === 'http://127.0.0.1'
      ? `${parsed.pathname}${parsed.search}${parsed.hash}`
      : '/'
  } catch {
    return '/'
  }
}

// 统一的 JSON 错误响应。
function reject(res, status, message) {
  const body = `${JSON.stringify({ error: message })}\n`
  res.writeHead(status, {
    'Cache-Control': 'no-store',
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  })
  res.end(body)
}

// 返回 true 表示请求已被处理（放行或拒绝），返回 false 表示无需鉴权、继续走后续路由。
export function authorizeDesktopRequest(req, res, url, { token, origin }) {
  // 未配置桌面令牌（如纯 Web 开发模式）时不做任何拦截。
  if (!token) return false

  // 引导路径：校验一次性 token 后种下 HttpOnly Cookie，并 302 跳回应用。
  if (url.pathname === DESKTOP_BOOTSTRAP_PATH) {
    if (req.method !== 'GET' || !secureEqual(url.searchParams.get('token'), token)) {
      reject(res, 401, 'Desktop bootstrap authentication failed.')
      return true
    }
    res.writeHead(302, {
      'Cache-Control': 'no-store',
      'Referrer-Policy': 'no-referrer',
      'Set-Cookie': `${DESKTOP_COOKIE_NAME}=${encodeURIComponent(token)}; HttpOnly; SameSite=Strict; Path=/`,
      Location: bootstrapLocation(url.searchParams.get('next')),
    })
    res.end()
    return true
  }

  // 常规请求：校验 Cookie 是否与令牌一致。
  const supplied = cookieValue(req.headers.cookie, DESKTOP_COOKIE_NAME)
  if (!secureEqual(supplied, encodeURIComponent(token))) {
    reject(res, 401, 'Desktop sidecar authentication is required.')
    return true
  }

  // 非幂等请求（写操作）再校验 Origin：Cookie 会被浏览器自动带上，
  // 若恶意站点对本地服务发起跨站请求，Origin 会与其不匹配而被拒绝。
  if (!['GET', 'HEAD', 'OPTIONS'].includes(req.method || 'GET')) {
    const requestOrigin = String(req.headers.origin || '')
    if (requestOrigin && requestOrigin !== origin) {
      reject(res, 403, 'Desktop sidecar origin check failed.')
      return true
    }
  }

  return false
}
