// Web 预览链接判定：点击外部（跨源）链接时交由预览面板打开。
// 同源链接交给当前页面正常导航，修饰键/下载/外部行为显式忽略；
// normalizeWebPreviewInput 负责把用户输入补全成带协议的可解析 URL。
export type PreviewLinkIntent = {
  href: string
  baseUrl: string
  button?: number
  download?: boolean
  altKey?: boolean
  ctrlKey?: boolean
  metaKey?: boolean
  shiftKey?: boolean
  behavior?: string | null
}

// 解析 Web 预览链接：基于 baseUrl 解析并只接受 http/https 协议，
// 其余协议（javascript: 等）返回 null 防止安全风险。
export function resolveWebPreviewUrl(href: string, baseUrl: string) {
  try {
    const url = new URL(href, baseUrl)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url : null
  } catch {
    return null
  }
}

// 规范化预览输入：无协议时补 https://（或 // 协议相对），再解析为绝对 URL。
export function normalizeWebPreviewInput(value: string, baseUrl: string) {
  const trimmed = value.trim()
  if (!trimmed) return null
  const candidate =
    /^[a-z][a-z\d+.-]*:/i.test(trimmed) || trimmed.startsWith('//') ? trimmed : `https://${trimmed}`
  return resolveWebPreviewUrl(candidate, baseUrl)?.href || null
}

// 判断链接点击是否应交给 Web 预览：仅“无修饰键的左键点击 + 跨源 URL”
// 才拦截；同源导航、下载、修饰键或显式 external/ignore 行为都不拦截。
export function shouldOpenWebPreview({
  href,
  baseUrl,
  button = 0,
  download = false,
  altKey = false,
  ctrlKey = false,
  metaKey = false,
  shiftKey = false,
  behavior,
}: PreviewLinkIntent) {
  if (
    button !== 0 ||
    download ||
    altKey ||
    ctrlKey ||
    metaKey ||
    shiftKey ||
    behavior === 'external' ||
    behavior === 'ignore'
  )
    return null

  const url = resolveWebPreviewUrl(href, baseUrl)
  if (!url) return null
  const base = resolveWebPreviewUrl(baseUrl, baseUrl)
  if (base && url.origin === base.origin) return null
  return url.href
}
