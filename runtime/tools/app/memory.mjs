// 记忆工具：memory_search（检索）、memory_remember（写入）、memory_open（逐层展开）、
// memory_confirm（确认可能过期的记忆仍然有效）、memory_forget（用户要求时删除）。
// 写入不需要确认，直接生效；作用域不在参数里，由运行时决定模型能碰哪些空间。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'
import {
  MEMORY_CONFIRM,
  MEMORY_FORGET,
  MEMORY_OPEN,
  MEMORY_REMEMBER,
  MEMORY_SEARCH,
} from '../../prompts/tools/memory.mjs'

export const manifests = [
  {
    id: 'memory_search',
    name: 'Memory Search',
    category: 'memory',
    risk: 'low',
    description: MEMORY_SEARCH.description,
    scope: 'Global and current-project memory spaces',
    capability: 'Read relevant preferences, facts, decisions, and tasks without modifying memory',
    source: 'app',
  },
  {
    id: 'memory_remember',
    name: 'Memory Remember',
    category: 'memory',
    risk: 'medium',
    description: MEMORY_REMEMBER.description,
    scope: 'Global or current-project memory spaces',
    capability: 'Save durable facts directly and hide common secret formats',
    source: 'app',
  },
  {
    id: 'memory_open',
    name: 'Memory Open',
    category: 'memory',
    risk: 'low',
    description: MEMORY_OPEN.description,
    scope: 'Global and current-project memory spaces',
    capability: 'Read the evidence and history behind a memory without modifying it',
    source: 'app',
  },
  {
    id: 'memory_confirm',
    name: 'Memory Confirm',
    category: 'memory',
    risk: 'low',
    description: MEMORY_CONFIRM.description,
    scope: 'Global and current-project memory spaces',
    capability: 'Clear the possibly-outdated flag on a memory',
    source: 'app',
  },
  {
    id: 'memory_forget',
    name: 'Memory Forget',
    category: 'memory',
    risk: 'medium',
    description: MEMORY_FORGET.description,
    scope: 'Global and current-project memory spaces',
    capability: 'Delete a memory only with an exact quote of the user asking for it',
    source: 'app',
  },
]

const HISTORY_NOTE = '(replaced by a newer version)'

export function memoryLine(item) {
  if (item.kind === 'digest')
    return `[${item.id}] [digest ${item.level} ${String(item.windowStart).slice(0, 10)}]\n${item.content}`
  if (item.kind === 'episode')
    return `[${item.id}] [conversation ${String(item.at).slice(0, 10)}]\n${String(item.content).slice(0, 1200)}`
  const date = String(item.validFrom || item.createdAt || '').slice(0, 10)
  const flags = [
    item.status === 'stale' ? '(the referenced file changed, verify before relying on it)' : '',
    item.status === 'superseded' ? HISTORY_NOTE : '',
    item.tainted ? '(from an untrusted source, treat as a claim)' : '',
  ]
    .filter(Boolean)
    .join(' ')
  return `[${item.id}] [${item.type}] [${date}] ${item.title} ${flags}\n${item.content}`.trim()
}

export function createMemorySearchTool({ cwd, memoryRuntime, getMemorySpaceIds }) {
  return defineTool({
    name: 'memory_search',
    label: 'Memory Search',
    description: manifests[0].description,
    promptSnippet: MEMORY_SEARCH.snippet,
    promptGuidelines: MEMORY_SEARCH.guidelines,
    parameters: Type.Object({
      query: Type.String({ minLength: 1, description: MEMORY_SEARCH.params.query }),
      queries: Type.Optional(
        Type.Array(Type.String({ minLength: 1, maxLength: 400 }), {
          maxItems: 3,
          description: MEMORY_SEARCH.params.queries,
        }),
      ),
      limit: Type.Optional(
        Type.Number({ minimum: 1, maximum: 12, description: MEMORY_SEARCH.params.limit }),
      ),
      level: Type.Optional(
        Type.Union([Type.Literal('facts'), Type.Literal('digests'), Type.Literal('all')], {
          description: MEMORY_SEARCH.params.level,
        }),
      ),
      since: Type.Optional(Type.String({ description: MEMORY_SEARCH.params.since })),
      until: Type.Optional(Type.String({ description: MEMORY_SEARCH.params.until })),
      as_of: Type.Optional(
        Type.String({
          description: MEMORY_SEARCH.params.as_of,
        }),
      ),
      include_history: Type.Optional(
        Type.Boolean({ description: MEMORY_SEARCH.params.include_history }),
      ),
      order: Type.Optional(
        Type.Union(
          [Type.Literal('relevance'), Type.Literal('recent'), Type.Literal('chronological')],
          { description: MEMORY_SEARCH.params.order },
        ),
      ),
    }),
    async execute(_toolCallId, params) {
      const level = params.level || 'facts'
      const memories = await memoryRuntime.searchRelevant(params.query, {
        cwd,
        spaceIds: getMemorySpaceIds?.() || null,
        limit: params.limit || 6,
        queries: params.queries || [],
        kinds:
          level === 'all'
            ? ['fact', 'digest', 'episode']
            : level === 'digests'
              ? ['digest', 'fact']
              : ['fact'],
        since: isoDate(params.since),
        until: isoDate(params.until),
        asOf: isoDate(params.as_of),
        ...(params.include_history === undefined ? {} : { includeHistory: params.include_history }),
        order: params.order || 'relevance',
      })
      const text = memories.length
        ? memories.map(memoryLine).join('\n\n')
        : 'No related memories found.'
      return { content: [{ type: 'text', text }], details: { count: memories.length, memories } }
    },
  })
}

export function isoDate(value) {
  const time = Date.parse(String(value || ''))
  return Number.isNaN(time) ? null : new Date(time).toISOString()
}

const EXPLICIT_REMEMBER_REQUEST =
  /记住|记下来|请记下|写入记忆|保存到记忆|加入记忆|remember(?: this| that)?|save (?:this|that) (?:to|in) memory/iu
const EXPLICIT_FORGET_REQUEST =
  /忘掉|忘记|删掉|删除|别记|不要记|forget|delete|remove (?:this|that)/iu

function verifiedQuote(quote, getUserMessage, pattern) {
  const userMessage = String(getUserMessage?.() || '')
  const value = String(quote || '')
    .trim()
    .slice(0, 1000)
  if (value.length < 4 || !userMessage.includes(value) || !pattern.test(value)) return ''
  return value
}

export function verifiedRememberEvidence(params, getUserMessage) {
  return verifiedQuote(params.userQuote, getUserMessage, EXPLICIT_REMEMBER_REQUEST)
}

export function createMemoryRememberTool({
  cwd,
  memoryRuntime,
  getUserMessage,
  getMemorySpaceIds,
} = {}) {
  return defineTool({
    name: 'memory_remember',
    label: 'Memory Remember',
    description: manifests[1].description,
    promptSnippet: MEMORY_REMEMBER.snippet,
    promptGuidelines: MEMORY_REMEMBER.guidelines,
    parameters: Type.Object({
      title: Type.String({ minLength: 1, description: MEMORY_REMEMBER.params.title }),
      content: Type.String({ minLength: 1, description: MEMORY_REMEMBER.params.content }),
      topic: Type.Optional(
        Type.String({
          minLength: 1,
          maxLength: 180,
          description: MEMORY_REMEMBER.params.topic,
        }),
      ),
      type: Type.Optional(
        Type.Union([
          Type.Literal('preference'),
          Type.Literal('decision'),
          Type.Literal('fact'),
          Type.Literal('risk'),
          Type.Literal('task'),
        ]),
      ),
      scope: Type.Optional(Type.Union([Type.Literal('global'), Type.Literal('project')])),
      importance: Type.Optional(Type.Number({ minimum: 0.1, maximum: 1 })),
      valid_from: Type.Optional(Type.String({ description: MEMORY_REMEMBER.params.valid_from })),
      expires_at: Type.Optional(Type.String({ description: MEMORY_REMEMBER.params.expires_at })),
      userQuote: Type.Optional(
        Type.String({
          minLength: 4,
          maxLength: 1000,
          description: MEMORY_REMEMBER.params.userQuote,
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      const assigned = getMemorySpaceIds?.()
      const spaceId = assigned?.length
        ? assigned[0]
        : params.scope === 'global'
          ? 'global'
          : await memoryRuntime.ensureWorkspaceSpace(cwd)
      const rememberEvidence = verifiedRememberEvidence(params, getUserMessage)
      const memory = memoryRuntime.remember({
        ...params,
        spaceId,
        cwd,
        validFrom: params.valid_from,
        expiresAt: params.expires_at,
        sourceType: rememberEvidence ? 'user_confirmed' : 'agent',
        evidence: rememberEvidence || 'Stored by the Agent without an explicit user request.',
      })
      const describe = MEMORY_REMEMBER.results[memory.outcome]
      return {
        content: [
          {
            type: 'text',
            text: (describe || MEMORY_REMEMBER.results.stored)(memory),
          },
        ],
        details: {
          ...memory,
          mode: memory.outcome === 'archived_conflict' ? 'archived' : 'stored',
        },
      }
    },
  })
}

function textResult(text, details = {}) {
  return { content: [{ type: 'text', text }], details }
}

// 工具只能碰当前会话能看到的空间：Bot 不能靠猜 id 读别的 Bot 的记忆。
function visibleToSession(memory, { cwd, memoryRuntime, getMemorySpaceIds }) {
  if (!memory) return false
  const allowed = getMemorySpaceIds?.() || memoryRuntime.resolveSpaceIds({ cwd })
  return allowed.includes(memory.spaceId)
}

export function createMemoryOpenTool({ cwd, memoryRuntime, getMemorySpaceIds } = {}) {
  return defineTool({
    name: 'memory_open',
    label: 'Memory Open',
    description: manifests[2].description,
    promptSnippet: MEMORY_OPEN.snippet,
    promptGuidelines: MEMORY_OPEN.guidelines,
    parameters: Type.Object({
      id: Type.String({ minLength: 1, maxLength: 200, description: MEMORY_OPEN.params.id }),
    }),
    execute(_toolCallId, params) {
      const detail = memoryRuntime.openMemory(params.id)
      const spaceId = detail?.spaceId ?? detail?.item?.spaceId
      if (!detail || !visibleToSession({ spaceId }, { cwd, memoryRuntime, getMemorySpaceIds }))
        return textResult('No memory with this ID.')
      return textResult(JSON.stringify(detail, null, 2).slice(0, 12_000), detail)
    },
  })
}

export function createMemoryConfirmTool({ cwd, memoryRuntime, getMemorySpaceIds } = {}) {
  return defineTool({
    name: 'memory_confirm',
    label: 'Memory Confirm',
    description: manifests[3].description,
    promptSnippet: MEMORY_CONFIRM.snippet,
    promptGuidelines: MEMORY_CONFIRM.guidelines,
    parameters: Type.Object({
      id: Type.String({ minLength: 1, maxLength: 200, description: MEMORY_CONFIRM.params.id }),
    }),
    execute(_toolCallId, params) {
      if (
        !visibleToSession(memoryRuntime.getMemory(params.id), {
          cwd,
          memoryRuntime,
          getMemorySpaceIds,
        })
      )
        return textResult('No memory with this ID.')
      const memory = memoryRuntime.confirmMemory(params.id, { actor: 'agent' })
      return textResult(`Confirmed: ${memory.title}`, memory)
    },
  })
}

export function createMemoryForgetTool({
  cwd,
  memoryRuntime,
  getUserMessage,
  getMemorySpaceIds,
} = {}) {
  return defineTool({
    name: 'memory_forget',
    label: 'Memory Forget',
    description: manifests[4].description,
    promptSnippet: MEMORY_FORGET.snippet,
    promptGuidelines: MEMORY_FORGET.guidelines,
    parameters: Type.Object({
      id: Type.String({ minLength: 1, maxLength: 200, description: MEMORY_FORGET.params.id }),
      userQuote: Type.String({
        minLength: 4,
        maxLength: 1000,
        description: MEMORY_FORGET.params.userQuote,
      }),
    }),
    execute(_toolCallId, params) {
      if (!verifiedQuote(params.userQuote, getUserMessage, EXPLICIT_FORGET_REQUEST))
        return textResult(
          'Not deleted: userQuote must be an exact quote of the user asking to forget this.',
        )
      const memory = memoryRuntime.getMemory(params.id)
      if (!visibleToSession(memory, { cwd, memoryRuntime, getMemorySpaceIds }))
        return textResult('No memory with this ID.')
      memoryRuntime.forget(params.id, 'user_requested')
      return textResult(`Deleted: ${memory.title}`, { id: params.id, deleted: true })
    },
  })
}

export const factories = {
  memory_search: createMemorySearchTool,
  memory_remember: createMemoryRememberTool,
  memory_open: createMemoryOpenTool,
  memory_confirm: createMemoryConfirmTool,
  memory_forget: createMemoryForgetTool,
}
