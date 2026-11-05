// 语义扩展：让模型给每条记忆写一行同义词、相关概念和另一种语言的说法，写进索引。
// 检索只匹配词面，这一行让“包管理器”也能搜到写着 npm 的记忆。量很小，允许退回主模型。
import { redactSecretText } from '../../security/secret-redaction.mjs'

const SUMMARY_SYSTEM_PROMPT = [
  'You expand a Pisper memory entry into semantic retrieval keywords so a later full-text search can find it even when the query uses synonyms, related concepts, different wording or the other language.',
  'For each memory, output one line of comma-separated keywords, aliases, related technologies, a short paraphrase, and the same key terms in both Chinese and English.',
  'Keep it dense and lowercase where natural. Do not invent facts that are not implied by the memory.',
  'Redact any secrets. Output exactly one line per memory. No numbering, no explanations, no JSON.',
].join('\n')

// 没有可用模型时退化为空摘要，不打断记忆写入。
export function createSemanticMemorySummarizer({ modelClient }) {
  return {
    async summarize(entries) {
      if (!(await modelClient?.available({ allowMain: true }))) return entries.map(() => '')
      const blocks = entries.map(
        (entry, index) =>
          `${index + 1}. ${String(entry?.title || '').slice(0, 140)}\n${String(entry?.content || '').slice(0, 1000)}`,
      )
      const result = await modelClient.complete({
        purpose: 'expand',
        allowMain: true,
        system: SUMMARY_SYSTEM_PROMPT,
        user: `Expand each memory into one semantic keyword line:\n\n${blocks.join('\n\n')}`,
        maxTokens: Math.min(1200, Math.max(400, entries.length * 120)),
      })
      if (result.errorCode) throw new Error(`语义扩展失败：${result.errorCode}`)
      const lines = result.text
        .split(/\r?\n/)
        .map((line) => line.replace(/^\s*\d+\.\s*/, '').trim())
        .filter(Boolean)
      return entries.map((_, index) => redactSecretText(String(lines[index] || '').slice(0, 2000)))
    },
  }
}
