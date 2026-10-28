import { translateText } from '@/app/i18n'
import { ApiError, requestJson } from '@/lib/http'
import { invalidResponseError } from '@/lib/http-response'

export type AssetKind = 'all' | 'image' | 'video' | 'file' | 'link'
export type AssetFilter = { query?: string; kind?: AssetKind; sessionId?: string | null }
export type Asset = {
  id: string
  name: string
  kind: string
  mimeType?: string
  size?: number
  url?: string
  source?: string
  sessionName?: string
}
export type AssetContent = Asset & {
  text?: string
  data?: string
  extension?: string
  truncated?: boolean
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
  return Object.fromEntries(Object.entries(value))
}
function text(value: unknown, required = false): string | undefined {
  if (value === undefined && !required) return undefined
  if (typeof value !== 'string' || (required && !value)) throw invalidResponseError()
  return value
}
export function safeAssetLink(value: unknown): string | undefined {
  if (typeof value !== 'string') return undefined
  try {
    const url = new URL(value)
    return ['http:', 'https:'].includes(url.protocol) ? url.href : undefined
  } catch {
    return undefined
  }
}
function parseAsset(value: unknown): Asset {
  const item = record(value)
  const id = text(item.id, true)!
  const name = text(item.name, true)!
  const kind = text(item.kind, true)!
  if (
    item.size !== undefined &&
    (typeof item.size !== 'number' || !Number.isFinite(item.size) || item.size < 0)
  )
    throw invalidResponseError()
  const url = item.url === undefined ? undefined : safeAssetLink(item.url)
  if ((item.url !== undefined && !url) || (kind === 'link' && !url)) throw invalidResponseError()
  return {
    id,
    name,
    kind,
    size: item.size as number | undefined,
    url,
    mimeType: text(item.mimeType),
    source: text(item.source),
    sessionName: text(item.sessionName),
  }
}
function parseContent(value: unknown, preview: boolean): AssetContent {
  const item = record(value)
  const asset = parseAsset(item)
  if (!['text', 'image', 'document', 'file'].includes(asset.kind)) throw invalidResponseError()
  if (asset.kind === 'text' && typeof item.text !== 'string') throw invalidResponseError()
  if (!preview && ['image', 'document'].includes(asset.kind) && typeof item.data !== 'string')
    throw invalidResponseError()
  if (item.truncated !== undefined && typeof item.truncated !== 'boolean')
    throw invalidResponseError()
  return {
    ...asset,
    text: text(item.text),
    data: text(item.data),
    extension: text(item.extension),
    truncated: item.truncated as boolean | undefined,
  }
}
export function normalizeAssetFilter(filter: AssetFilter = {}) {
  return {
    query: (filter.query || '').trim(),
    kind: filter.kind || 'all',
    sessionId: filter.sessionId || '',
  }
}
const assetPath = (id: string) => `/api/assets/${encodeURIComponent(id)}`
export const assetMediaUrl = (id: string) => `${assetPath(id)}/download?inline=1`

export const assetsApi = {
  async list(filter: AssetFilter = {}, signal?: AbortSignal) {
    const { query, kind, sessionId } = normalizeAssetFilter(filter)
    const params = new URLSearchParams()
    if (query) params.set('query', query)
    // Runtime 将视频登记为 file，视频类型在同域边界根据 MIME 派生。
    if (kind !== 'all') params.set('kind', kind === 'video' ? 'file' : kind)
    if (sessionId) params.set('sessionId', sessionId)
    return requestJson(`/api/assets?${params}`, {
      signal,
      parse(value) {
        const data = record(value)
        if (!Array.isArray(data.assets)) throw invalidResponseError()
        const assets = data.assets.map(parseAsset)
        return kind === 'video'
          ? assets.filter((asset) => asset.mimeType?.startsWith('video/'))
          : assets
      },
    })
  },
  preview: (id: string, signal?: AbortSignal) =>
    requestJson(`${assetPath(id)}/content?preview=1`, {
      signal,
      parse: (value) => parseContent(value, true),
    }),
  content: (id: string, signal?: AbortSignal) =>
    requestJson(`${assetPath(id)}/content`, {
      signal,
      parse: (value) => parseContent(value, false),
    }),
  async createLink(input: { name: string; url: string }) {
    const url = safeAssetLink(input.url.trim())
    if (!url)
      throw new ApiError(translateText('assets:assetsPage.invalidLink'), {
        kind: 'protocol',
        data: { code: 'INVALID_ASSET_URL' },
      })
    return requestJson('/api/assets', {
      method: 'POST',
      body: { kind: 'link', name: input.name, url, source: 'upload' },
      parse: parseAsset,
    })
  },
  remove: (id: string) =>
    requestJson(assetPath(id), {
      method: 'DELETE',
      parse(value) {
        if (record(value).deleted !== true) throw invalidResponseError()
        return true
      },
    }),
}
