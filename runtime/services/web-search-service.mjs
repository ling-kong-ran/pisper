// Web 搜索服务：通过 Bing RSS 实现网页搜索，无 API Key 依赖；
// 提供配置归一化、结果解析与可用性测试。
import { readJson } from '../storage/json-file.mjs'

export const DEFAULT_WEB_SEARCH_CONFIG = Object.freeze({
  provider: 'bing',
  language: 'auto',
  safeSearch: 1,
  maxResults: 8,
})

const BING_RSS_URL = 'https://www.bing.com/search'
const SAFE_SEARCH = { 0: 'off', 1: 'moderate', 2: 'strict' }
const MARKETS = {
  'zh-CN': { mkt: 'zh-CN', setlang: 'zh' },
  'zh-TW': { mkt: 'zh-TW', setlang: 'zh-Hant' },
  'en-US': { mkt: 'en-US', setlang: 'en' },
  'en-GB': { mkt: 'en-GB', setlang: 'en' },
  'ja-JP': { mkt: 'ja-JP', setlang: 'ja' },
  'ko-KR': { mkt: 'ko-KR', setlang: 'ko' },
}

function boundedInteger(value, fallback, minimum, maximum) {
  const number = Number(value)
  return Number.isFinite(number)
    ? Math.min(maximum, Math.max(minimum, Math.round(number)))
    : fallback
}

export function normalizeWebSearchConfig(input = {}) {
  return {
    provider: 'bing',
    language:
      String(input.language || 'auto')
        .trim()
        .slice(0, 40) || 'auto',
    safeSearch: boundedInteger(input.safeSearch, 1, 0, 2),
    maxResults: boundedInteger(input.maxResults, 8, 1, 12),
  }
}

function decodeXml(value) {
  return String(value || '')
    .replace(/^<!\[CDATA\[|\]\]>$/g, '')
    .replace(/&#x([0-9a-f]+);/gi, (_match, code) => String.fromCodePoint(Number.parseInt(code, 16)))
    .replace(/&#(\d+);/g, (_match, code) => String.fromCodePoint(Number(code)))
    .replace(/&nbsp;/gi, ' ')
    .replace(/&amp;/gi, '&')
    .replace(/&lt;/gi, '<')
    .replace(/&gt;/gi, '>')
    .replace(/&quot;/gi, '"')
    .replace(/&#39;|&apos;/gi, "'")
}

function plainText(value, maximum = 1200) {
  return decodeXml(value)
    .replace(/<[^>]*>/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, maximum)
}

function xmlTag(block, name) {
  return block.match(new RegExp(`<${name}>([\\s\\S]*?)<\\/${name}>`, 'i'))?.[1] || ''
}

export function parseBingRssResults(xml, limit = 8) {
  return [...String(xml || '').matchAll(/<item>([\s\S]*?)<\/item>/gi)]
    .flatMap((match) => {
      const rawUrl = plainText(xmlTag(match[1], 'link'), 2000)
      let url
      try {
        url = new URL(rawUrl)
      } catch {
        return []
      }
      if (!['http:', 'https:'].includes(url.protocol)) return []
      return [
        {
          title: plainText(xmlTag(match[1], 'title'), 300) || url.hostname,
          url: url.toString(),
          snippet: plainText(xmlTag(match[1], 'description')),
          publishedAt: plainText(xmlTag(match[1], 'pubDate'), 120),
        },
      ]
    })
    .slice(0, limit)
}

function resultText(query, results) {
  if (!results.length) return `Bing 没有找到“${query}”的结果。`
  return [
    `Bing 搜索结果：${query}`,
    ...results.map((result, index) =>
      [
        `${index + 1}. ${result.title}`,
        result.url,
        result.snippet || '无摘要',
        result.publishedAt ? `发布时间：${result.publishedAt}` : '',
      ]
        .filter(Boolean)
        .join('\n'),
    ),
  ].join('\n\n')
}

export class WebSearchService {
  constructor({ configPath, fetchImpl = globalThis.fetch } = {}) {
    this.configPath = configPath
    this.fetchImpl = fetchImpl
  }

  async getConfig() {
    const appConfig = this.configPath ? await readJson(this.configPath, {}) : {}
    return normalizeWebSearchConfig(appConfig.webSearch || {})
  }

  async search(input, { signal, config } = {}) {
    const settings = config ? normalizeWebSearchConfig(config) : await this.getConfig()
    const query = String(input?.query || '')
      .trim()
      .slice(0, 500)
    if (!query) throw new Error('搜索关键词不能为空。')
    const limit = boundedInteger(input.limit, settings.maxResults, 1, 12)
    const page = boundedInteger(input.page, 1, 1, 20)
    const endpoint = new URL(BING_RSS_URL)
    endpoint.searchParams.set('q', query)
    endpoint.searchParams.set('format', 'rss')
    endpoint.searchParams.set('count', String(limit))
    endpoint.searchParams.set('adlt', SAFE_SEARCH[settings.safeSearch])
    if (page > 1) endpoint.searchParams.set('first', String((page - 1) * limit + 1))
    const market = MARKETS[input.language || settings.language]
    if (market) {
      endpoint.searchParams.set('mkt', market.mkt)
      endpoint.searchParams.set('setlang', market.setlang)
    }

    let response
    try {
      const timeoutSignal = AbortSignal.timeout(15_000)
      response = await this.fetchImpl(endpoint, {
        headers: {
          Accept: 'application/rss+xml,application/xml,text/xml',
          'User-Agent': 'Pisper Web Search/1.0',
        },
        redirect: 'follow',
        signal: signal ? AbortSignal.any([signal, timeoutSignal]) : timeoutSignal,
      })
    } catch (error) {
      if (signal?.aborted) throw error
      if (error?.name === 'TimeoutError' || error?.name === 'AbortError')
        throw new Error('Bing 搜索请求超时，请检查网络后重试。')
      throw new Error(`无法连接 Bing：${error instanceof Error ? error.message : String(error)}`)
    }

    const body = await response.text()
    if (!response.ok)
      throw new Error(
        `Bing 搜索失败（HTTP ${response.status}）：${plainText(body, 300) || '无响应内容'}`,
      )
    if (!/<rss\b/i.test(body)) throw new Error('Bing 没有返回可解析的 RSS 搜索结果，请稍后重试。')
    const results = parseBingRssResults(body, limit)
    return {
      query,
      provider: 'bing',
      results,
      text: resultText(query, results),
    }
  }

  test(config) {
    return this.search({ query: 'Pisper AI agent', limit: 3 }, { config })
  }
}
