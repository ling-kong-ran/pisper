// 协议探测：访问站点根路径检测是否为 New API 网关（带缓存与超时），
// 用于驱动分发前确定协议。
const detectionCache = new Map()

function rootUrl(baseUrl) {
  const url = new URL(baseUrl)
  url.pathname = '/'
  url.search = ''
  url.hash = ''
  return url.href
}

async function detectNewAPI(baseUrl) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), 3500)
  try {
    const response = await fetch(rootUrl(baseUrl), {
      headers: { Accept: 'text/html,application/json' },
      signal: controller.signal,
    })
    const text = await response.text()
    return (
      response.ok &&
      /(?:<title>\s*New API\s*<\/title>|Unified AI API gateway|QuantumNous)/i.test(text)
    )
  } catch {
    return false
  } finally {
    clearTimeout(timer)
  }
}

export function isNewAPIProvider(baseUrl) {
  let key
  try {
    key = new URL(baseUrl).origin.toLowerCase()
  } catch {
    return Promise.resolve(false)
  }
  if (!detectionCache.has(key)) detectionCache.set(key, detectNewAPI(baseUrl))
  return detectionCache.get(key)
}

export function clearVisualProtocolDetectionCache() {
  detectionCache.clear()
}
