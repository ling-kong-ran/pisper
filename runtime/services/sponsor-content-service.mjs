// 赞助内容服务：从仓库远端拉取并校验 docs/sponsors.json（赞助商/活动文案），
// 本地缓存并附带本地兜底文件；供前端展示社区赞助内容。
import { mkdir, readFile, rename, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { DEFAULT_BRANCH, REPOSITORY_API } from '../../shared/app-update.mjs'

const CACHE_VERSION = 1
const DEFAULT_CACHE_MS = 15 * 60_000
const MAX_DOCUMENT_BYTES = 256 * 1024
const MAX_CAMPAIGNS = 50
const REMOTE_PATH = 'docs/sponsors.json'
const PLACEMENT_PATTERN = /^[a-z0-9][a-z0-9-]{0,79}$/
const CAMPAIGN_ID_PATTERN = /^[a-z0-9][a-z0-9-]{0,79}$/
const SUPPORTED_LOCALES = ['zh-CN', 'en-US']

function requiredString(value, field, maxLength) {
  const result = String(value || '').trim()
  if (!result || result.length > maxLength) throw new Error(`${field} 无效。`)
  return result
}

function localizedText(value, field, maxLength) {
  if (typeof value === 'string') {
    const text = requiredString(value, field, maxLength)
    return { 'zh-CN': text, 'en-US': text }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${field} 无效。`)
  }
  const result = {}
  for (const locale of SUPPORTED_LOCALES) {
    if (value[locale] != null) result[locale] = requiredString(value[locale], field, maxLength)
  }
  const fallback = result['zh-CN'] || result['en-US']
  if (!fallback) throw new Error(`${field} 无效。`)
  for (const locale of SUPPORTED_LOCALES) result[locale] ||= fallback
  return result
}

function httpsUrl(value) {
  const text = requiredString(value, 'campaign.href', 2_048)
  const url = new URL(text)
  if (url.protocol !== 'https:' || url.username || url.password) {
    throw new Error('campaign.href 必须是无凭据的 HTTPS 地址。')
  }
  return url.href
}

function optionalDate(value, field) {
  if (value == null || value === '') return ''
  const timestamp = Date.parse(String(value))
  if (!Number.isFinite(timestamp)) throw new Error(`${field} 无效。`)
  return new Date(timestamp).toISOString()
}

export function validateSponsorDocument(value) {
  if (!value || typeof value !== 'object' || Number(value.schemaVersion) !== 1) {
    throw new Error('赞助配置版本不受支持。')
  }
  if (!Array.isArray(value.campaigns) || value.campaigns.length > MAX_CAMPAIGNS) {
    throw new Error('赞助配置数量无效。')
  }
  const ids = new Set()
  const campaigns = value.campaigns.map((item) => {
    if (!item || typeof item !== 'object' || Array.isArray(item)) {
      throw new Error('赞助配置条目无效。')
    }
    const id = requiredString(item.id, 'campaign.id', 80).toLowerCase()
    const placement = requiredString(item.placement, 'campaign.placement', 80).toLowerCase()
    if (!CAMPAIGN_ID_PATTERN.test(id) || ids.has(id)) throw new Error('campaign.id 无效或重复。')
    if (!PLACEMENT_PATTERN.test(placement)) throw new Error('campaign.placement 无效。')
    ids.add(id)
    const startsAt = optionalDate(item.startsAt, 'campaign.startsAt')
    const endsAt = optionalDate(item.endsAt, 'campaign.endsAt')
    if (startsAt && endsAt && Date.parse(endsAt) <= Date.parse(startsAt)) {
      throw new Error('campaign 时间范围无效。')
    }
    const priority = item.priority == null ? 0 : Number(item.priority)
    if (!Number.isInteger(priority) || priority < -1_000 || priority > 1_000) {
      throw new Error('campaign.priority 无效。')
    }
    return {
      id,
      placement,
      enabled: item.enabled !== false,
      priority,
      name: localizedText(item.name, 'campaign.name', 80),
      description: localizedText(item.description, 'campaign.description', 240),
      href: httpsUrl(item.href),
      startsAt,
      endsAt,
    }
  })
  return { schemaVersion: 1, campaigns }
}

function emptyDocument() {
  return { schemaVersion: 1, campaigns: [] }
}

function localeText(value, locale) {
  return value[locale] || value['zh-CN'] || value['en-US'] || ''
}

function activeCampaign(campaign, now) {
  if (!campaign.enabled) return false
  if (campaign.startsAt && Date.parse(campaign.startsAt) > now) return false
  if (campaign.endsAt && Date.parse(campaign.endsAt) <= now) return false
  return true
}

async function readJson(path) {
  try {
    return JSON.parse(await readFile(path, 'utf8'))
  } catch {
    return null
  }
}

export class SponsorContentService {
  constructor({
    dataDir,
    fallbackPath = '',
    appVersion = '',
    branch = DEFAULT_BRANCH,
    fetcher = globalThis.fetch,
    now = () => Date.now(),
    cacheMs = DEFAULT_CACHE_MS,
  } = {}) {
    this.cachePath = join(dataDir, 'sponsors-cache.json')
    this.fallbackPath = fallbackPath
    this.appVersion = String(appVersion || '').trim() || 'web'
    this.branch = branch
    this.fetcher = fetcher
    this.now = now
    this.cacheMs = cacheMs
    this.state = null
    this.initialized = false
    this.initializing = null
    this.pending = null
    this.lastAttemptAt = 0
  }

  async init() {
    if (this.initialized) return
    if (!this.initializing) {
      this.initializing = this.loadInitialState().finally(() => {
        this.initialized = true
        this.initializing = null
      })
    }
    await this.initializing
  }

  async loadInitialState() {
    const cached = await readJson(this.cachePath)
    try {
      if (cached?.version === CACHE_VERSION) {
        this.state = {
          document: validateSponsorDocument(cached.document),
          etag: String(cached.etag || ''),
          checkedAt: String(cached.checkedAt || ''),
          source: 'cache',
        }
        return
      }
    } catch {
      // Ignore invalid cache and fall back to the configuration shipped with the app.
    }
    const fallback = this.fallbackPath ? await readJson(this.fallbackPath) : null
    try {
      if (fallback) {
        this.state = {
          document: validateSponsorDocument(fallback),
          etag: '',
          checkedAt: '',
          source: 'bundled',
        }
        return
      }
    } catch {
      // A malformed bundled file must not prevent Pisper from starting.
    }
    this.state = { document: emptyDocument(), etag: '', checkedAt: '', source: 'empty' }
  }

  isFresh() {
    const checkedAt = Date.parse(this.state?.checkedAt || '')
    const newestCheck = Math.max(Number.isFinite(checkedAt) ? checkedAt : 0, this.lastAttemptAt)
    return newestCheck > 0 && this.now() - newestCheck < this.cacheMs
  }

  async persist() {
    const temporaryPath = `${this.cachePath}.${process.pid}.tmp`
    await mkdir(dirname(this.cachePath), { recursive: true })
    await writeFile(
      temporaryPath,
      `${JSON.stringify(
        {
          version: CACHE_VERSION,
          etag: this.state.etag,
          checkedAt: this.state.checkedAt,
          document: this.state.document,
        },
        null,
        2,
      )}\n`,
      'utf8',
    )
    await rename(temporaryPath, this.cachePath)
  }

  async refresh({ force = false } = {}) {
    await this.init()
    if (!force && this.isFresh()) return
    if (this.pending) return this.pending
    this.lastAttemptAt = this.now()
    this.pending = this.fetchRemote().finally(() => {
      this.pending = null
    })
    return this.pending
  }

  async fetchRemote() {
    const headers = {
      Accept: 'application/vnd.github.raw+json',
      'X-GitHub-Api-Version': '2022-11-28',
      'User-Agent': `Pisper/${this.appVersion}`,
    }
    if (this.state?.etag) headers['If-None-Match'] = this.state.etag
    const response = await this.fetcher(
      `${REPOSITORY_API}/contents/${REMOTE_PATH}?ref=${encodeURIComponent(this.branch)}`,
      {
        headers,
        signal:
          typeof AbortSignal !== 'undefined' && typeof AbortSignal.timeout === 'function'
            ? AbortSignal.timeout(5_000)
            : undefined,
      },
    )
    if (response.status === 304) {
      this.state.checkedAt = new Date(this.now()).toISOString()
      await this.persist()
      return
    }
    if (!response.ok) throw new Error(`GitHub 赞助配置读取失败：HTTP ${response.status}`)
    const declaredSize = Number(response.headers.get('content-length') || 0)
    if (declaredSize > MAX_DOCUMENT_BYTES) throw new Error('赞助配置文件过大。')
    const text = await response.text()
    if (!text || Buffer.byteLength(text, 'utf8') > MAX_DOCUMENT_BYTES) {
      throw new Error('赞助配置文件为空或过大。')
    }
    const document = validateSponsorDocument(JSON.parse(text))
    this.state = {
      document,
      etag: String(response.headers.get('etag') || ''),
      checkedAt: new Date(this.now()).toISOString(),
      source: 'remote',
    }
    await this.persist()
  }

  async getPlacement(placement, { locale = 'zh-CN', refresh = false } = {}) {
    await this.init()
    const normalizedPlacement = requiredString(placement, 'placement', 80).toLowerCase()
    if (!PLACEMENT_PATTERN.test(normalizedPlacement)) throw new Error('赞助位名称无效。')
    await this.refresh({ force: refresh }).catch(() => {})
    const selectedLocale = SUPPORTED_LOCALES.includes(locale) ? locale : 'zh-CN'
    const now = this.now()
    const campaigns = this.state.document.campaigns
      .filter(
        (campaign) => campaign.placement === normalizedPlacement && activeCampaign(campaign, now),
      )
      .sort((left, right) => right.priority - left.priority || left.id.localeCompare(right.id))
      .map((campaign) => ({
        id: campaign.id,
        name: localeText(campaign.name, selectedLocale),
        description: localeText(campaign.description, selectedLocale),
        href: campaign.href,
      }))
    return {
      placement: normalizedPlacement,
      campaigns,
      checkedAt: this.state.checkedAt || null,
      source: this.state.source,
    }
  }
}
