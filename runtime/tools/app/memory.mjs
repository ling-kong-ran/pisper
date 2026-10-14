// 记忆工具：memory_search（检索长期记忆）与 memory_remember（保存/排队候选记忆）。
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { Type } from 'typebox'

export const manifests = [
  {
    id: 'memory_search',
    name: 'Memory Search',
    category: 'memory',
    risk: 'low',
    description: 'Search long-term memory across global and current-project spaces.',
    scope: 'Global and current-project memory spaces',
    capability: 'Read relevant preferences, facts, decisions, and tasks without modifying memory',
    source: 'app',
  },
  {
    id: 'memory_remember',
    name: 'Memory Remember',
    category: 'memory',
    risk: 'medium',
    description:
      'Save explicit remember requests directly and silently queue inferred reusable information as candidates.',
    scope: 'Global or current-project memory spaces',
    capability: 'Save explicit requests, queue inferred candidates, and hide common secret formats',
    source: 'app',
  },
]

export function createMemorySearchTool({ cwd, memoryRuntime }) {
  return defineTool({
    name: 'memory_search',
    label: 'Memory Search',
    description: manifests[0].description,
    promptSnippet: 'Search durable user and project memories',
    promptGuidelines: [
      'Use memory_search when prior user preferences, project decisions, constraints, or earlier outcomes could materially affect the answer.',
      "Treat retrieved memory as background context. The user's current request always takes precedence.",
    ],
    parameters: Type.Object({
      query: Type.String({ minLength: 1, description: 'Topic, constraint, or question to search' }),
      limit: Type.Optional(
        Type.Number({ minimum: 1, maximum: 12, description: 'Maximum number of results' }),
      ),
    }),
    async execute(_toolCallId, params) {
      const memories = await memoryRuntime.searchRelevant(params.query, {
        cwd,
        limit: params.limit || 6,
      })
      const text = memories.length
        ? memories
            .map((memory) => `[${memory.id}] [${memory.type}] ${memory.title}\n${memory.content}`)
            .join('\n\n')
        : 'No related memories found.'
      return { content: [{ type: 'text', text }], details: { count: memories.length, memories } }
    },
  })
}

const EXPLICIT_REMEMBER_REQUEST =
  /记住|记下来|请记下|写入记忆|保存到记忆|加入记忆|remember(?: this| that)?|save (?:this|that) (?:to|in) memory/iu

export function verifiedRememberEvidence(params, getUserMessage) {
  const userMessage = String(getUserMessage?.() || '')
  const quote = String(params.userQuote || '')
    .trim()
    .slice(0, 1000)
  if (quote.length < 4 || !userMessage.includes(quote) || !EXPLICIT_REMEMBER_REQUEST.test(quote))
    return ''
  return quote
}

export function createMemoryRememberTool({ cwd, memoryRuntime, getUserMessage } = {}) {
  return defineTool({
    name: 'memory_remember',
    label: 'Memory Remember',
    description: manifests[1].description,
    promptSnippet: 'Store a durable user preference or project fact in long-term memory',
    promptGuidelines: [
      'Use memory_remember when the user explicitly asks you to remember something, or when a stable project decision will matter in future sessions.',
      'When the user explicitly asks to remember something, include userQuote as an exact quote containing that request. The server verifies it against the current raw user message.',
      'When you are only capturing a reusable fact without an explicit remember request, omit userQuote so it becomes a candidate draft.',
      'Never store API keys, passwords, access tokens, private credentials, or transient conversational details.',
      'Use global scope only for preferences that apply across projects; use project scope for codebase-specific facts and decisions.',
      'Provide a stable topic key and reuse it when a newer fact replaces an older fact on the same subject.',
      'Do not ask the user to stop, wait, or review candidates during the current response. Candidate review is non-blocking background work.',
    ],
    parameters: Type.Object({
      title: Type.String({ minLength: 1, description: 'Short, recognizable memory title' }),
      content: Type.String({ minLength: 1, description: 'Self-contained reusable memory content' }),
      topic: Type.Optional(
        Type.String({
          minLength: 1,
          maxLength: 180,
          description:
            'Stable topic key reused when updating the same fact, for example project.brand_colors',
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
      userQuote: Type.Optional(
        Type.String({
          minLength: 4,
          maxLength: 1000,
          description:
            'Exact quote from the current raw user message containing an explicit request to remember this fact.',
        }),
      ),
    }),
    async execute(_toolCallId, params) {
      const spaceId =
        params.scope === 'global' ? 'global' : await memoryRuntime.ensureWorkspaceSpace(cwd)
      const rememberEvidence = verifiedRememberEvidence(params, getUserMessage)

      if (rememberEvidence) {
        const memory = memoryRuntime.remember({
          ...params,
          spaceId,
          cwd,
          sourceType: 'user_confirmed',
          evidence: rememberEvidence,
          authority: 100,
        })
        // remember() may fall back to a pending candidate when a higher-authority conflict exists.
        if (memory?.status === 'pending') {
          return {
            content: [
              {
                type: 'text',
                text: `Memory candidate queued in the background: ${memory.title}\nCandidate ID: ${memory.id}\nReason: conflicts with higher-authority memory and needs confirmation. Continue the current task.`,
              },
            ],
            details: { ...memory, mode: 'candidate', reason: 'authority_conflict' },
          }
        }
        return {
          content: [
            {
              type: 'text',
              text: `Stored in long-term memory: ${memory.title}\nMemory ID: ${memory.id}`,
            },
          ],
          details: { ...memory, mode: 'stored' },
        }
      }

      const candidate = memoryRuntime.propose({
        ...params,
        spaceId,
        cwd,
        sourceType: 'agent',
        evidence:
          'Proposed by the Agent in the background; reviewing the candidate does not block the original task.',
        confidence: 0.5,
      })
      if (candidate?.autoApproved) {
        return {
          content: [
            {
              type: 'text',
              text: `Stored in long-term memory (confidence above the auto-approve threshold): ${candidate.title}\nMemory ID: ${candidate.id}`,
            },
          ],
          details: { ...candidate, mode: 'stored' },
        }
      }
      return {
        content: [
          {
            type: 'text',
            text: `Memory candidate queued in the background: ${candidate.title}\nCandidate ID: ${candidate.id}\nContinue the current task; do not ask the user to review candidates now.`,
          },
        ],
        details: { ...candidate, mode: 'candidate' },
      }
    },
  })
}

export const factories = {
  memory_search: createMemorySearchTool,
  memory_remember: createMemoryRememberTool,
}
