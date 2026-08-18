// 对话记忆抽取：识别用户消息中的记忆意图，调用模型抽取候选记忆。
// 消息先脱敏、evidence 必须逐字来自原文，防止密钥入库与模型编造。
import { containsSecretText, redactSecretText } from '../../security/secret-redaction.mjs'

function textContent(content) {
  if (typeof content === 'string') return content
  return Array.isArray(content)
    ? content
        .filter((part) => part?.type === 'text')
        .map((part) => part.text || '')
        .join('')
    : ''
}

function parseJsonArray(value) {
  const text = String(value || '')
    .trim()
    .replace(/^```(?:json)?\s*/i, '')
    .replace(/\s*```$/i, '')
  const start = text.indexOf('[')
  const end = text.lastIndexOf(']')
  if (start < 0 || end <= start) return []
  try {
    const result = JSON.parse(text.slice(start, end + 1))
    return Array.isArray(result) ? result : []
  } catch {
    return []
  }
}

function exactEvidence(value, user, assistant) {
  const evidence = String(value || '')
    .trim()
    .slice(0, 1000)
  if (!evidence) return ''
  return String(user || '').includes(evidence) || String(assistant || '').includes(evidence)
    ? evidence
    : ''
}

// 是否应抽取：消息足够长且命中记忆相关关键词（记住/偏好/决定/不再使用等）。
export function shouldExtractConversationMemory(user, _assistant = '') {
  const value = String(user || '').trim()
  if (value.length < 8) return false
  return /记住|记下来|请记下|以后|从今以后|今后|长期|偏好|习惯|约定|决定|确认采用|确认使用|最终方案|暂时不引入|不再使用|改用|改为|不要再|remember|preference|from now on|we decided|use instead/iu.test(
    value,
  )
}

// 抽取候选记忆：跳过无意图的消息；结果限制 3 条并清洗字段；
// evidence 必须逐字出现在原文中，且不含脱敏标记。
export async function extractConversationMemories({ modelRuntime, model, user, assistant }) {
  if (!modelRuntime || !model || !shouldExtractConversationMemory(user, assistant))
    return { memories: [], usage: null, timestamp: Date.now() }
  const sourceHadSecrets = containsSecretText(user) || containsSecretText(assistant)
  const safeUser = redactSecretText(String(user || '')).slice(0, 2400)
  const safeAssistant = redactSecretText(String(assistant || '')).slice(0, 3600)
  const result = await modelRuntime.completeSimple(
    model,
    {
      systemPrompt: [
        'You propose memory candidates for Pisper. Candidates are reviewed by the user before becoming trusted memory.',
        'Keep only reusable and relatively stable information explicitly supported by the source conversation.',
        'Allowed: lasting user preferences, explicit constraints, confirmed project architecture or technical decisions, and recurring risks.',
        'Do not treat the assistant claiming that work is complete, tests passed, or a fact is true as verified evidence.',
        'Never store temporary questions, small talk, speculation, plans in progress, API keys, passwords, tokens, private keys, or other secrets.',
        'Output only a JSON array with at most 3 items. Output [] when there is nothing worth proposing.',
        'Every item must include an exact short evidence quote copied verbatim from either the user or assistant message.',
        'Use a narrow stable topic key. A topic groups the same individual fact; do not use broad topics such as project.architecture.',
        'Use the source conversation language for title and content.',
        'Item format: {"title":"short title","content":"self-contained candidate fact","topic":"narrow stable topic key","type":"preference|decision|fact|risk|task","scope":"global|project","importance":0.1 to 1,"confidence":0 to 1,"evidence":"exact source quote"}',
      ].join('\n'),
      messages: [
        {
          role: 'user',
          content: `用户消息：\n${safeUser}\n\nAgent 回复：\n${safeAssistant}`,
          timestamp: Date.now(),
        },
      ],
    },
    {
      ...(model.reasoning ? { reasoning: 'low' } : { temperature: 0.1 }),
      maxTokens: 900,
    },
  )
  if (result.errorMessage) return { memories: [], usage: result.usage, timestamp: result.timestamp }
  const memories = parseJsonArray(textContent(result.content))
    .slice(0, 3)
    .flatMap((item) => {
      const title = String(item?.title || '')
        .trim()
        .slice(0, 140)
      const content = String(item?.content || '')
        .trim()
        .slice(0, 4000)
      const evidence = exactEvidence(item?.evidence, safeUser, safeAssistant)
      if (!title || !content || !evidence || evidence.includes('[REDACTED SECRET]')) return []
      return [
        {
          title: redactSecretText(title),
          content: redactSecretText(content),
          topic: String(item?.topic || '')
            .trim()
            .slice(0, 180),
          type: ['preference', 'decision', 'fact', 'risk', 'task'].includes(item.type)
            ? item.type
            : 'fact',
          scope: item.scope === 'global' ? 'global' : 'project',
          importance: Math.min(1, Math.max(0.1, Number(item.importance) || 0.5)),
          confidence: Math.min(1, Math.max(0, Number(item.confidence) || 0.5)),
          evidence,
        },
      ]
    })
  return { memories, usage: result.usage, timestamp: result.timestamp, sourceHadSecrets }
}
