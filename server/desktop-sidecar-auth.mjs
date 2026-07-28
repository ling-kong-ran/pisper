import { timingSafeEqual } from 'node:crypto'

export const DESKTOP_BOOTSTRAP_PATH = '/_pisper/desktop/bootstrap'
export const DESKTOP_COOKIE_NAME = '__pisper_desktop'

function secureEqual(left, right) {
  const leftBytes = Buffer.from(String(left || ''))
  const rightBytes = Buffer.from(String(right || ''))
  return leftBytes.length === rightBytes.length && timingSafeEqual(leftBytes, rightBytes)
}

function cookieValue(header, name) {
  for (const entry of String(header || '').split(';')) {
    const separator = entry.indexOf('=')
    if (separator < 0) continue
    if (entry.slice(0, separator).trim() === name) return entry.slice(separator + 1).trim()
  }
  return ''
}

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

function reject(res, status, message) {
  const body = `${JSON.stringify({ error: message })}\n`
  res.writeHead(status, {
    'Cache-Control': 'no-store',
    'Content-Type': 'application/json; charset=utf-8',
    'Content-Length': Buffer.byteLength(body),
  })
  res.end(body)
}

export function authorizeDesktopRequest(req, res, url, { token, origin }) {
  if (!token) return false

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

  const supplied = cookieValue(req.headers.cookie, DESKTOP_COOKIE_NAME)
  if (!secureEqual(supplied, encodeURIComponent(token))) {
    reject(res, 401, 'Desktop sidecar authentication is required.')
    return true
  }

  if (!['GET', 'HEAD', 'OPTIONS'].includes(req.method || 'GET')) {
    const requestOrigin = String(req.headers.origin || '')
    if (requestOrigin && requestOrigin !== origin) {
      reject(res, 403, 'Desktop sidecar origin check failed.')
      return true
    }
  }

  return false
}
