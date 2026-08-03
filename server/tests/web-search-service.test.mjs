import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { ToolPluginService } from '../services/tool-plugin-service.mjs'
import { parseBingRssResults, WebSearchService } from '../services/web-search-service.mjs'
import { createAppTools, TOOL_CATALOG, TOOL_PRESETS } from '../tools/registry.mjs'

const SEARCH_XML = `<?xml version="1.0" encoding="utf-8"?>
  <rss version="2.0"><channel>
    <item><title>Pisper &amp; Agent</title><link>https://example.com/pisper</link><description>An open agent.</description><pubDate>Tue, 21 Jul 2026 03:14:00 GMT</pubDate></item>
    <item><title>Documentation</title><link>https://example.com/docs</link><description><![CDATA[Official <b>docs</b>]]></description></item>
    <item><title>Unsafe</title><link>javascript:alert(1)</link><description>discard me</description></item>
  </channel></rss>`

test('Bing RSS search builds a lightweight request and parses structured results', async () => {
  let request
  const service = new WebSearchService({
    fetchImpl: async (url, options) => {
      request = { url: new URL(url), options }
      return new Response(SEARCH_XML, { status: 200, headers: { 'Content-Type': 'text/xml' } })
    },
  })

  const result = await service.search(
    {
      query: 'Pisper agent',
      language: 'en-US',
      page: 2,
      limit: 2,
    },
    { config: { language: 'auto', safeSearch: 2, maxResults: 8 } },
  )

  assert.equal(request.url.hostname, 'www.bing.com')
  assert.equal(request.url.searchParams.get('q'), 'Pisper agent')
  assert.equal(request.url.searchParams.get('format'), 'rss')
  assert.equal(request.url.searchParams.get('count'), '2')
  assert.equal(request.url.searchParams.get('adlt'), 'strict')
  assert.equal(request.url.searchParams.get('first'), '3')
  assert.equal(request.url.searchParams.get('mkt'), 'en-US')
  assert.equal(request.url.searchParams.get('setlang'), 'en')
  assert.equal(request.options.redirect, 'follow')
  assert.equal(result.provider, 'bing')
  assert.equal(result.results.length, 2)
  assert.equal(result.results[0].title, 'Pisper & Agent')
  assert.equal(result.results[0].url, 'https://example.com/pisper')
  assert.equal(result.results[0].publishedAt, 'Tue, 21 Jul 2026 03:14:00 GMT')
  assert.equal(result.results[1].snippet, 'Official docs')
  assert.match(result.text, /https:\/\/example\.com\/pisper/)
})

test('Bing RSS parser removes unsafe links and reports invalid responses', async () => {
  assert.equal(parseBingRssResults(SEARCH_XML, 10).length, 2)
  const invalid = new WebSearchService({
    fetchImpl: async () => new Response('<html>Temporarily unavailable</html>', { status: 200 }),
  })
  await assert.rejects(invalid.search({ query: 'test' }), /没有返回可解析的 RSS/)
})

test('web_search needs no endpoint, belongs to read-only presets, and persists preferences', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-web-search-'))
  const path = join(directory, 'pisper.json')
  t.after(() => rm(directory, { recursive: true, force: true }))
  const plugins = new ToolPluginService(path)
  const saved = await plugins.saveState({
    enabledTools: ['read', 'web_search'],
    webSearch: { language: 'zh-CN', safeSearch: 1, maxResults: 6 },
  })
  assert.ok(TOOL_CATALOG.some((tool) => tool.id === 'web_search' && tool.source === 'app'))
  assert.ok(TOOL_PRESETS['read-only'].includes('web_search'))
  assert.ok(saved.enabledTools.includes('web_search'))
  assert.deepEqual(saved.webSearch, {
    provider: 'bing',
    language: 'zh-CN',
    safeSearch: 1,
    maxResults: 6,
  })

  let searched
  const [tool] = createAppTools({
    enabledTools: ['web_search'],
    webSearchService: {
      search: async (input) => {
        searched = input
        return { text: 'result', results: [] }
      },
    },
  })
  const output = await tool.execute('search-1', { query: 'latest Pisper release', limit: 5 })
  assert.equal(searched.query, 'latest Pisper release')
  assert.equal(output.content[0].text, 'result')
})
