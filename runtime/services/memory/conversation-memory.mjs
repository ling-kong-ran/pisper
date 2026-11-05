// 对话记忆抽取：用户明确表达记忆意图时立即抽取；其余轮次由后台按会话批量抽取。
// 消息先脱敏、evidence 必须逐字来自原文，防止密钥入库与模型编造。
import { containsSecretText, redactSecretText } from '../../security/secret-redaction.mjs'
import { awaitMemoryOperation } from './abortable-memory-operation.mjs'

const REDACTED_MARK = '[REDACTED SECRET]'
const MEMORY_ITEM_TYPES = ['preference', 'decision', 'fact', 'risk', 'task']

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
  if (start < 0 || end <= start) return null
  try {
    const result = JSON.parse(text.slice(start, end + 1))
    return Array.isArray(result) ? result : null
  } catch {
    return null
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

// 模型写的时间能解析成 ISO 才要。还没发生的生效时间、已经过去的到期时间都没有意义，当作没写。
function parseFactTimes(item, now) {
  const parsed = (value) => {
    const time = Date.parse(String(value || ''))
    return Number.isNaN(time) ? null : time
  }
  const validFrom = parsed(item?.valid_from)
  const expiresAt = parsed(item?.expires_at)
  return {
    validFrom: validFrom !== null && validFrom <= now ? new Date(validFrom).toISOString() : '',
    expiresAt: expiresAt !== null && expiresAt > now ? new Date(expiresAt).toISOString() : '',
  }
}

// 两条路径共用的字段清洗：标题和内容为空、证据不成立的条目在这之前已被丢弃。
function cleanMemoryItem(item, title, content, now) {
  return {
    title: redactSecretText(title),
    content: redactSecretText(content),
    topic: String(item?.topic || '')
      .trim()
      .slice(0, 180),
    type: MEMORY_ITEM_TYPES.includes(item?.type) ? item.type : 'fact',
    scope: item?.scope === 'global' ? 'global' : 'project',
    importance: Math.min(1, Math.max(0.1, Number(item?.importance) || 0.5)),
    ...parseFactTimes(item, now),
  }
}

const itemTitle = (item) =>
  String(item?.title || '')
    .trim()
    .slice(0, 140)
const itemContent = (item) =>
  String(item?.content || '')
    .trim()
    .slice(0, 4000)

const FACT_TIME_RULES = [
  'Add "valid_from" (ISO date) only when the conversation states since when something has been true, resolved against the date of that turn. Otherwise leave it out.',
  'Add "expires_at" (ISO date) only when the conversation says it is temporary, such as "use X this week". Otherwise leave it out.',
]

// 是否应抽取：消息足够长且命中记忆相关关键词（记住/偏好/决定/不再使用等）。
export function shouldExtractConversationMemory(user, _assistant = '') {
  const value = String(user || '').trim()
  if (value.length < 8) return false
  return /记住|记下来|请记下|以后|从今以后|今后|长期|偏好|习惯|约定|决定|确认采用|确认使用|最终方案|暂时不引入|不再使用|改用|改为|不要再|remember|preference|from now on|we decided|use instead/iu.test(
    value,
  )
}

// 抽取记忆：跳过无意图的消息；结果限制 3 条并清洗字段；
// evidence 必须逐字出现在原文中，且不含脱敏标记。
export async function extractConversationMemories({
  modelRuntime,
  model,
  user,
  assistant,
  signal,
}) {
  if (!shouldExtractConversationMemory(user, assistant))
    return { memories: [], usage: null, timestamp: Date.now() }
  const failure = (errorCode, response) => ({
    memories: [],
    usage: response?.usage || null,
    timestamp: response?.timestamp || Date.now(),
    errorCode,
  })
  if (!modelRuntime || !model) return failure('model_unavailable')
  if (signal?.aborted) return failure('aborted')
  const sourceHadSecrets = containsSecretText(user) || containsSecretText(assistant)
  const safeUser = redactSecretText(String(user || '')).slice(0, 2400)
  const safeAssistant = redactSecretText(String(assistant || '')).slice(0, 3600)
  let result
  try {
    result = await awaitMemoryOperation(
      modelRuntime.completeSimple(
        model,
        {
          systemPrompt: [
            'You extract long-term memories for Pisper. They take effect immediately without user review, so only keep what you are sure of.',
            'Keep only reusable and relatively stable information explicitly supported by the source conversation.',
            'Allowed: lasting user preferences, explicit constraints, confirmed project architecture or technical decisions, and recurring risks.',
            'Do not treat the assistant claiming that work is complete, tests passed, or a fact is true as verified evidence.',
            'Never store temporary questions, small talk, speculation, plans in progress, API keys, passwords, tokens, private keys, or other secrets.',
            'Output only a JSON array with at most 3 items. Output [] when there is nothing worth proposing.',
            'Every item must include an exact short evidence quote copied verbatim from either the user or assistant message.',
            'Use a narrow stable topic key. A topic groups the same individual fact; do not use broad topics such as project.architecture.',
            'Use the source conversation language for title and content.',
            ...FACT_TIME_RULES,
            'Item format: {"title":"short title","content":"self-contained fact","topic":"narrow stable topic key","type":"preference|decision|fact|risk|task","scope":"global|project","importance":0.1 to 1,"confidence":0 to 1,"evidence":"exact source quote","valid_from":"optional ISO date","expires_at":"optional ISO date"}',
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
          // 推理与正文可能共享输出上限；900 tokens 会让 GLM 等模型仅产生思考。
          maxTokens: Math.min(
            model.reasoning ? 8192 : 2048,
            Number(model.maxTokens) > 0 ? Number(model.maxTokens) : Infinity,
          ),
          signal,
        },
      ),
      signal,
    )
  } catch {
    // SDK 错误可能含请求、密钥或原文；只向捕获层返回稳定诊断码。
    return failure(signal?.aborted ? 'aborted' : 'model_error')
  }
  if (signal?.aborted || result?.stopReason === 'aborted') return failure('aborted', result)
  if (result?.errorMessage || result?.stopReason === 'error') return failure('model_error', result)
  if (result?.stopReason === 'length') return failure('token_limit', result)
  const text = textContent(result?.content)
  if (!text.trim()) return failure('empty_response', result)
  const items = parseJsonArray(text)
  if (!items) return failure('invalid_response', result)
  const now = Date.now()
  const memories = items.slice(0, 3).flatMap((item) => {
    const title = itemTitle(item)
    const content = itemContent(item)
    const evidence = exactEvidence(item?.evidence, safeUser, safeAssistant)
    if (!title || !content || !evidence || evidence.includes(REDACTED_MARK)) return []
    return [
      {
        ...cleanMemoryItem(item, title, content, now),
        confidence: Math.min(1, Math.max(0, Number(item.confidence) || 0.5)),
        evidence,
        // 证据只出现在助手的话里时，权威度按推断算，不能当成用户说的。
        evidenceRole: safeUser.includes(evidence) ? 'user' : 'assistant',
      },
    ]
  })
  return { memories, usage: result.usage, timestamp: result.timestamp, sourceHadSecrets }
}

// 一批最多给模型看这么多字：成本按字数算，长对话平摊到每轮，用户的话多留一些。
const BATCH_PROMPT_CHARS = 24_000
const MIN_TURN_CHARS = 400
const MIN_EVIDENCE_CHARS = 4

const BATCH_SYSTEM_PROMPT = [
  'You read a stretch of conversation between a user and an assistant and extract long-term memories for Pisper.',
  'Memories take effect immediately. Nobody reviews them, so if you are not sure, leave it out. An empty result is normal and fine.',
  'Keep only reusable and relatively stable information explicitly supported by the conversation: lasting user preferences, facts about the user, explicit constraints, confirmed project architecture or technical decisions, and recurring risks.',
  'Do not treat the assistant claiming that work is complete, tests passed, or a fact is true as verified evidence.',
  'Never store temporary questions, small talk, speculation, plans in progress, API keys, passwords, tokens, private keys, or other secrets.',
  'Every item must include an exact short evidence quote copied verbatim from one turn. Prefer quoting what the user said.',
  'Use a narrow stable topic key. A topic groups the same individual fact; do not use broad topics such as project.architecture.',
  'Use the source conversation language for title and content, and make each content self-contained.',
  ...FACT_TIME_RULES,
  'Output only a JSON array. Item format: {"title":"short title","content":"self-contained fact","topic":"narrow stable topic key","type":"preference|decision|fact|risk|task","scope":"global|project","importance":0.1 to 1,"evidence":"exact source quote","valid_from":"optional ISO date","expires_at":"optional ISO date"}',
].join('\n')

function batchPrompt(turns) {
  const perTurn = Math.max(MIN_TURN_CHARS, Math.floor(BATCH_PROMPT_CHARS / turns.length))
  return turns
    .map((turn, index) => {
      const user = String(turn.user || '').slice(0, Math.floor(perTurn * 0.6))
      const assistant = String(turn.assistant || '').slice(0, perTurn - user.length)
      const date = String(turn.at || '').slice(0, 10)
      return `第 ${index + 1} 轮（${date}）\n用户：\n${user}\n\nAgent：\n${assistant}`
    })
    .join('\n\n=====\n\n')
}

// 证据在哪一轮、是谁说的由这里查，不信模型的说法。同一句话两边都有时算用户说的。
function locateEvidence(evidence, turns) {
  const fromUser = turns.find((turn) => String(turn.user || '').includes(evidence))
  if (fromUser) return { turn: fromUser, evidenceRole: 'user' }
  const fromAssistant = turns.find((turn) => String(turn.assistant || '').includes(evidence))
  return fromAssistant ? { turn: fromAssistant, evidenceRole: 'assistant' } : null
}

/**
 * 批量抽取。turns 是 L0 原文（已脱敏），每条形如 { id, user, assistant, at, cwd, spaceId }。
 * 只走辅助模型；失败时 errorCode 是稳定诊断码，memories 为空。
 * @returns {Promise<{ memories: Array<object>, errorCode?: string }>}
 */
export async function extractBatchMemories({ modelClient, turns, signal, now = Date.now() }) {
  if (!turns?.length) return { memories: [] }
  if (!modelClient) return { memories: [], errorCode: 'model_unavailable' }
  let result
  try {
    result = await awaitMemoryOperation(
      modelClient.complete({
        purpose: 'extract',
        allowMain: false,
        system: BATCH_SYSTEM_PROMPT,
        user: batchPrompt(turns),
        maxTokens: 2048,
        signal,
      }),
      signal,
    )
  } catch {
    return { memories: [], errorCode: signal?.aborted ? 'aborted' : 'model_error' }
  }
  if (signal?.aborted) return { memories: [], errorCode: 'aborted' }
  if (result?.errorCode) return { memories: [], errorCode: result.errorCode }
  const items = parseJsonArray(result?.text)
  if (!items) return { memories: [], errorCode: 'invalid_response' }
  // 轮数多可说的多，但一批的条数要有封顶：模型列一长串多半是在凑数。
  const itemLimit = Math.min(12, 3 + Math.floor(turns.length / 3))
  const memories = items.slice(0, itemLimit).flatMap((item) => {
    const title = itemTitle(item)
    const content = itemContent(item)
    const evidence = String(item?.evidence || '')
      .trim()
      .slice(0, 1000)
    if (!title || !content || Array.from(evidence).length < MIN_EVIDENCE_CHARS) return []
    if (evidence.includes(REDACTED_MARK)) return []
    const located = locateEvidence(evidence, turns)
    if (!located) return []
    return [
      {
        ...cleanMemoryItem(item, title, content, now),
        evidence,
        evidenceRole: located.evidenceRole,
        episodeId: located.turn.id,
        sourceTimestamp: located.turn.at,
        cwd: located.turn.cwd || '',
        // 项目记忆归证据所在那一轮的空间：一个会话中途可能换过工作目录。
        projectSpaceId: located.turn.spaceId,
      },
    ]
  })
  return { memories }
}
