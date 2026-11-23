// 语义扩展：让模型给每条记忆写一行同义词、相关概念和另一种语言的说法，写进索引。
// 检索只匹配词面，这一行让“包管理器”也能搜到写着 npm 的记忆。量很小，允许退回主模型。
import { redactSecretText } from '../../security/secret-redaction.mjs'
import { pisperError } from '../../../shared/errors.mjs'
import { SEMANTIC_EXPANSION_SYSTEM, semanticExpansionUser } from '../../prompts/memory.mjs'
import { parseModelJson } from './memory-model-client.mjs'

// 没有可用模型时退化为空摘要，不打断记忆写入。
export function createSemanticMemorySummarizer({ modelClient }) {
  return {
    async summarize(entries, { signal } = {}) {
      if (!(await modelClient?.available({ allowMain: true }))) return entries.map(() => '')
      const blocks = entries.map((entry, index) => ({
        id: index + 1,
        title: String(entry.title || '').slice(0, 140),
        content: String(entry.content || '').slice(0, 1000),
      }))
      const result = await modelClient.complete({
        purpose: 'expand',
        allowMain: true,
        signal,
        system: SEMANTIC_EXPANSION_SYSTEM,
        user: semanticExpansionUser(blocks),
        maxTokens: Math.min(1200, Math.max(400, entries.length * 120)),
      })
      if (result.errorCode === 'model_unavailable' || result.errorCode === 'budget_exhausted')
        return entries.map(() => '')
      if (result.errorCode)
        throw pisperError('failed', 'memory', `semantic expansion: ${result.errorCode}`)
      const items = parseModelJson(result.text, '[', ']')
      if (!Array.isArray(items)) throw pisperError('failed', 'memory', 'invalid expansion response')
      const keywords = new Map()
      for (const item of items) {
        const id = item?.id
        if (!Number.isInteger(id) || id < 1 || id > entries.length) continue
        // 缺项留待补算；编号重复时两条都不能确定归属。
        const text = typeof item.keywords === 'string' ? item.keywords.trim() : ''
        keywords.set(id, keywords.has(id) ? '' : redactSecretText(text.slice(0, 2000)))
      }
      return blocks.map(({ id }) => keywords.get(id) || '')
    },
  }
}
