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

export function resolveWebPreviewUrl(href: string, baseUrl: string) {
  try {
    const url = new URL(href, baseUrl)
    return url.protocol === 'http:' || url.protocol === 'https:' ? url : null
  } catch {
    return null
  }
}

export function normalizeWebPreviewInput(value: string, baseUrl: string) {
  const trimmed = value.trim()
  if (!trimmed) return null
  const candidate =
    /^[a-z][a-z\d+.-]*:/i.test(trimmed) || trimmed.startsWith('//') ? trimmed : `https://${trimmed}`
  return resolveWebPreviewUrl(candidate, baseUrl)?.href || null
}

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
