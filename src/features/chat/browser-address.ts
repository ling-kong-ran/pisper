// 右侧浏览器地址栏：输入的是网址就打开，否则当成搜索词交给默认搜索引擎。
// 只判断字面形状，不查域名表：带点、不含空格、最后一段像顶级域的才算主机，其余一律搜索，
// 这样“v1.2”“什么是 rust”这类输入不会被当成打不开的网址。
import { normalizeWebPreviewInput, resolveWebPreviewUrl } from '@/lib/web-preview'

// 默认搜索引擎：国内外都能直接打开，不需要登录。
export const BROWSER_SEARCH_URL = 'https://www.bing.com/search?q='

export type BrowserAddress = { kind: 'url' | 'search'; url: string }

function isIpv4(host: string) {
  const parts = host.split('.')
  return parts.length === 4 && parts.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)
}

// 地址里主机那一段（去掉端口、路径）。带 @ 的多半是邮箱，不当主机。
function hostOf(text: string) {
  const authority = text.split(/[/?#]/, 1)[0] ?? ''
  if (!authority || authority.includes('@')) return null
  if (authority.startsWith('['))
    return /^\[[\da-f:.]+\](?::\d{1,5})?$/i.test(authority) ? authority : null
  const match = /^([^:]+)(?::(\d{1,5}))?$/.exec(authority)
  return match ? match[1] : null
}

type HostShape = 'local' | 'public' | null

function hostShape(text: string): HostShape {
  if (/\s/.test(text)) return null
  const host = hostOf(text)
  if (!host) return null
  const lower = host.toLowerCase()
  // 本机、IP 地址一般是开发服务，没有证书，按 http 打开。
  if (
    lower.startsWith('[') ||
    lower === 'localhost' ||
    lower.endsWith('.localhost') ||
    isIpv4(lower)
  )
    return 'local'
  const labels = lower.split('.')
  if (labels.length < 2 || labels.some((label) => !/^[\p{L}\p{N}-]+$/u.test(label))) return null
  const tld = labels[labels.length - 1]
  return /^(?:\p{L}{2,}|xn--[a-z\d-]+)$/u.test(tld) ? 'public' : null
}

export function browserSearchUrl(query: string) {
  return `${BROWSER_SEARCH_URL}${encodeURIComponent(query)}`
}

// 返回要打开的地址；明确写了 http(s) 却解析不出来时返回 null，由调用方提示地址无效。
export function resolveBrowserAddress(input: string, baseUrl: string): BrowserAddress | null {
  const text = input.trim()
  if (!text) return null
  if (/^https?:/i.test(text) || text.startsWith('//')) {
    const url = normalizeWebPreviewInput(text, baseUrl)
    return url ? { kind: 'url', url } : null
  }
  const shape = hostShape(text)
  if (shape === 'local') {
    const url = resolveWebPreviewUrl(`http://${text}`, baseUrl)?.href
    if (url) return { kind: 'url', url }
  } else if (shape === 'public') {
    // 局域网的 .local 等由 normalizeWebPreviewInput 改用 http。
    const url = normalizeWebPreviewInput(text, baseUrl)
    if (url) return { kind: 'url', url }
  }
  return { kind: 'search', url: browserSearchUrl(text) }
}
