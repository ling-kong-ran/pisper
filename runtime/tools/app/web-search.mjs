// Web 搜索工具：无需 API Key 的 Bing RSS 搜索。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import { WEB_SEARCH } from '../../prompts/tools/web-search.mjs'

export const manifest = {
  id: 'web_search',
  name: 'Web Search',
  category: 'search',
  // 只读的公开搜索：只发送搜索词，不带用户凭据、不改动远端状态，按低风险处理，需审批模式下也不逐次确认。
  risk: 'low',
  description: WEB_SEARCH.description,
  scope: 'Bing public web search',
  capability:
    'Send search terms and return titles, links, summaries, and dates without modifying web pages',
  source: 'app',
}

export function createWebSearchTool({ webSearchService }) {
  return defineTool({
    name: manifest.id,
    label: manifest.name,
    description: manifest.description,
    promptSnippet: WEB_SEARCH.snippet,
    promptGuidelines: WEB_SEARCH.guidelines,
    parameters: Type.Object({
      query: Type.String({
        minLength: 1,
        maxLength: 500,
        description: WEB_SEARCH.params.query,
      }),
      language: Type.Optional(
        Type.String({ maxLength: 40, description: WEB_SEARCH.params.language }),
      ),
      page: Type.Optional(
        Type.Number({ minimum: 1, maximum: 20, description: WEB_SEARCH.params.page }),
      ),
      limit: Type.Optional(
        Type.Number({ minimum: 1, maximum: 12, description: WEB_SEARCH.params.limit }),
      ),
    }),
    async execute(_toolCallId, params, signal, onUpdate) {
      if (!webSearchService) throw new Error('Web search service is not initialized.')
      onUpdate?.({ content: [{ type: 'text', text: `Searching Bing for: ${params.query}` }] })
      const result = await webSearchService.search(params, { signal })
      return { content: [{ type: 'text', text: result.text }], details: result }
    },
  })
}
