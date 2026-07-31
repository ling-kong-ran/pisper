import { redactSecretText } from '../../security/secret-redaction.mjs'

const SUMMARY_SYSTEM_PROMPT = [
  'You expand a Pisper memory entry into semantic retrieval keywords so a later full-text search can find it even when the query uses synonyms, related concepts, or different wording.',
  'For each memory, output one line of comma-separated keywords, aliases, related technologies, and a short paraphrase.',
  'Keep it dense and lowercase where natural. Do not invent facts that are not implied by the memory.',
  'Redact any secrets. Output exactly one line per memory, in the memory language. No numbering, no explanations, no JSON.',
].join('\n')

function textFromContent(content) {
  if (typeof content === 'string') return content
  return Array.isArray(content)
    ? content.filter((part) => part?.type === 'text').map((part) => part.text || '').join('')
    : ''
}

export function createSemanticMemorySummarizer({ getModelRuntime, getDefaultModel }) {
  return {
    async summarize(entries) {
      const modelRuntime = getModelRuntime?.()
      const model = getDefaultModel?.()
      if (!modelRuntime || !model) return entries.map(() => '')
      const blocks = entries.map((entry, index) => `${index + 1}. ${String(entry?.title || '').slice(0, 140)}\n${String(entry?.content || '').slice(0, 1000)}`)
      const result = await modelRuntime.completeSimple(model, {
        systemPrompt: SUMMARY_SYSTEM_PROMPT,
        messages: [{
          role: 'user',
          content: `Expand each memory into one semantic keyword line:\n\n${blocks.join('\n\n')}`,
          timestamp: Date.now(),
        }],
      }, {
        ...(model.reasoning ? { reasoning: 'low' } : { temperature: 0.1 }),
        maxTokens: Math.min(1200, Math.max(400, entries.length * 120)),
      })
      if (result.errorMessage) throw new Error(result.errorMessage)
      const lines = textFromContent(result.content).split(/\r?\n/).map((line) => line.replace(/^\s*\d+\.\s*/, '').trim()).filter(Boolean)
      return entries.map((_, index) => redactSecretText(cleanSummary(lines[index] || '')))
    },
  }
}

function cleanSummary(value) {
  return String(value || '').slice(0, 2000)
}
