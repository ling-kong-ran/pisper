// 记忆工具（memory_search / remember / open / confirm / forget）的说明。

export const MEMORY_CONFIRM = Object.freeze({
  description:
    'Mark a memory flagged stale as still correct, after verifying it against the current files.',
  snippet: 'Mark a stale memory as still correct after verifying it',
  guidelines: [
    'Use memory_confirm only after checking a memory marked stale against the current files. If it is wrong, store the corrected fact with memory_remember instead.',
  ],
  params: {
    id: 'Memory ID',
  },
})

export const MEMORY_FORGET = Object.freeze({
  description:
    'Permanently delete a memory and its older versions when the user explicitly asks to forget it.',
  snippet: 'Delete a memory the user explicitly asked to forget',
  guidelines: [
    'Use memory_forget only when the user explicitly asks to forget or delete something. Include userQuote as an exact quote of that request; the server verifies it against the current raw user message.',
  ],
  params: {
    id: 'Memory ID',
    userQuote: 'Exact quote from the current raw user message asking to forget this.',
  },
})

export const MEMORY_OPEN = Object.freeze({
  description:
    'Expand a memory search result: a fact to its evidence, version history and source conversation; a digest to the facts behind it; a session digest also to its turns.',
  snippet: 'Expand a memory search result to its evidence and history',
  guidelines: [
    'Use memory_open after memory_search when you need the evidence, the version history, or the original conversation behind a result.',
  ],
  params: {
    id: 'ID from memory_search',
  },
})

export const MEMORY_REMEMBER = Object.freeze({
  description:
    'Save a durable fact to long-term memory. It takes effect immediately and replaces an older fact on the same topic.',
  snippet: 'Store a durable user preference or project fact in long-term memory',
  guidelines: [
    'Use memory_remember when the user explicitly asks you to remember something, or when a stable project decision will matter in future sessions.',
    'When the user explicitly asks to remember something, include userQuote as an exact quote containing that request. The server verifies it against the current raw user message.',
    'Memories take effect immediately. There is no review step, so do not ask the user to confirm or review them.',
    'Never store API keys, passwords, access tokens, private credentials, or transient conversational details.',
    'Use global scope only for preferences that apply across projects; use project scope for codebase-specific facts and decisions.',
    'Provide a stable topic key and reuse it when a newer fact replaces an older fact on the same subject.',
    'Set valid_from when the user says since when something has been true, and expires_at for things that only hold temporarily.',
  ],
  params: {
    title: 'Short, recognizable memory title',
    content: 'Self-contained reusable memory content',
    topic: 'Stable topic key reused when updating the same fact, for example project.brand_colors',
    valid_from: 'ISO date since when this has been true, if not just now',
    expires_at: 'ISO date after which this no longer holds',
    userQuote:
      'Exact quote from the current raw user message containing an explicit request to remember this fact.',
  },
  // memory_remember 按写入结果回给模型的话。
  results: {
    reinforced: (memory) =>
      `Already in long-term memory, refreshed: ${memory.title}\nMemory ID: ${memory.id}`,
    superseded: (memory) =>
      `Stored in long-term memory, replacing the older version on this topic: ${memory.title}\nMemory ID: ${memory.id}`,
    archived_conflict: (memory) =>
      `Not applied: this conflicts with something the user stated directly, which stays in effect. Archived in case the user wants it: ${memory.title}\nMemory ID: ${memory.id}\nContinue the current task.`,
    forgotten: () =>
      'Not stored: the user deleted this memory earlier. Do not try to store it again.',
    stored: (memory) => `Stored in long-term memory: ${memory.title}\nMemory ID: ${memory.id}`,
  },
})

export const MEMORY_SEARCH = Object.freeze({
  description:
    'Search long-term memory across global and current-project spaces, by relevance or by time, including how a fact changed.',
  snippet: 'Search durable user and project memories',
  guidelines: [
    'Use memory_search when prior user preferences, project decisions, constraints, or earlier outcomes could materially affect the answer.',
    'Pass queries with up to 3 rewordings, including the other language (Chinese and English): retrieval matches words, not meaning.',
    'Use since/until or order for questions about time ("what changed last week"). Use include_history or as_of for how something used to be.',
    'Use level "digests" to find what a past conversation or period was about.',
    "Treat retrieved memory as background context. The user's current request always takes precedence.",
  ],
  params: {
    query: 'Topic, constraint, or question to search',
    queries: 'Extra rewordings of the query, for example in the other language',
    limit: 'Maximum number of results',
    level:
      'facts (default): distilled facts. digests: summaries by session, day, week and month. all: also past conversations.',
    since: 'Only results from this ISO date on',
    until: 'Only results before this ISO date',
    as_of: 'ISO date: what was true at that moment, not what is true now',
    include_history: 'Also return facts that a newer version has replaced',
    order: 'relevance (default), recent first, or oldest first',
  },
})
