// Bot 的对话（v2）：对话只负责说话和做决定，自己不跑工具；活由运行时按“决定”派给子 agent 在后台并行跑。
// 这里是纯函数：对话的系统提示、每轮附带的状态快照、决定请求的说明与解析。运行时接线在 agent-runtime-facade。

export const BOT_MAX_PARALLEL_AGENTS = 3
const MAX_DECISIONS = 6
const MAX_TITLE_CHARS = 80
const MAX_TASK_CHARS = 8_000
const MAX_ADD_CHARS = 4_000
const TIERS = new Set(['fast', 'standard', 'strong'])
const MEMORY_SOURCES = new Set(['told', 'correction', 'review'])
const MAX_MEMORY_CHARS = 500
const SNAPSHOT_OUTPUT_CHARS = 300

// 对话的系统提示：它是谁、它负责什么、它怎么干活（派子 agent，不自己动手）。
export function botConversationPrompt(bot) {
  const parts = [
    `你是 ${bot.name}，用户在 Pisper 里的一个长期助手。这是你和用户的一段持续对话：前文都算数，消息也可能来自用户的聊天软件或定时任务。`,
    [
      '你是经理，不是工人：你自己不能调用任何工具，不读文件、不跑命令。',
      '要动手的事由系统按你的回复派子 agent 去做：在回复里用一两句话说清你要派人做什么，不要假装自己已经做了。',
      '对已经在做的活有补充或要停掉，也在回复里说明，系统会照办。',
      '问进度、问结果、闲聊：直接回答，不用派活。每条消息后面附有 [当前状态]，列出你手上的子 agent 与它们的进展。',
      '子 agent 做完的结果会作为一条消息回到这里：用一两句话向用户汇报结论，不要整段照抄。',
      '能自己判断的就判断，不要为了确认而停下来问；只有缺少只有用户知道的信息、要用户拍板时，才问用户。',
    ].join('\n'),
  ]
  const instructions = String(bot.instructions || '').trim()
  if (instructions) parts.push(`[${bot.name} 的工作说明]\n${instructions}`)
  return parts.join('\n\n')
}

function clip(value, max) {
  const text = String(value ?? '')
    .replace(/\s+/g, ' ')
    .trim()
  return text.length > max ? `${text.slice(0, max)}…` : text
}

// 每轮附带的状态快照（不显示给人看）：手上的子 agent、各自状态与最近进展、是否在等批准。
export function botStateSnapshot({
  agents = [],
  waitingApprovals = 0,
  limit = BOT_MAX_PARALLEL_AGENTS,
} = {}) {
  const active = agents.filter((agent) => ['queued', 'starting', 'running'].includes(agent.status))
  const recent = agents
    .filter((agent) => !['queued', 'starting', 'running'].includes(agent.status))
    .slice(-5)
  const line = (agent) => {
    const progress = clip(agent.output || agent.error || '', SNAPSHOT_OUTPUT_CHARS)
    return `- [${agent.id.slice(0, 8)}] ${agent.title || agent.taskName} · ${agent.status}${progress ? ` · ${progress}` : ''}`
  }
  const rows = [
    `[当前状态]`,
    `同时最多 ${limit} 个子 agent；现在在做 ${active.length} 个${waitingApprovals ? `，${waitingApprovals} 个操作在等用户批准` : ''}。`,
  ]
  if (active.length) rows.push('在做：', ...active.map(line))
  if (recent.length) rows.push('最近结束：', ...recent.map(line))
  if (!active.length && !recent.length) rows.push('手上没有活。')
  return rows.join('\n')
}

// 决定请求的说明：这一轮对话之后，要执行哪些决定（JSON）。
export function botDecisionInstruction({
  agents = [],
  models = [],
  limit = BOT_MAX_PARALLEL_AGENTS,
} = {}) {
  const active = agents.filter((agent) => ['queued', 'starting', 'running'].includes(agent.status))
  return [
    '<pisper_decide>',
    'Based on the conversation above and your latest reply, decide what the system should do now.',
    'Only act on what the conversation clearly needs and what your reply said; do not start work nobody asked for, and do not repeat work a sub-agent is already doing.',
    'Respond with JSON only, no prose: {"actions": [...]}; use an empty list when nothing needs doing.',
    'Action types:',
    '- {"type":"spawn","title":"<short title in the user\'s language>","task":"<self-contained instructions: goal, relevant paths, what to report back>","model":"<provider/model from the list, optional>","tier":"fast|standard|strong (optional, used when no model)"}',
    '- {"type":"add","agent":"<sub-agent id prefix>","text":"<what to add; it is delivered after its current step>"}',
    '- {"type":"stop","agent":"<sub-agent id prefix>"}',
    '- {"type":"remember","text":"<one short durable fact, preference or rule, in the user\'s language>","source":"told|correction|review"}',
    '- {"type":"forget","memory":"<memory id from [你记得的事]>"}',
    'Remember only what will still matter later: what the user asked you to remember, corrections ("以后别…"), and lessons from finished work (source review). Do not remember one-off task details. Forget what the user says is wrong or no longer true.',
    `At most ${limit} sub-agents run at once; extra spawns wait in a queue.`,
    'Pick the model by what the task needs (coding, long documents, images, quick cheap chores); leave it out when unsure.',
    `Running sub-agents: ${active.length ? active.map((agent) => `${agent.id.slice(0, 8)} "${agent.title || agent.taskName}"`).join(', ') : 'none'}.`,
    `Available models: ${models.length ? models.join(', ') : 'only the default'}.`,
    '</pisper_decide>',
  ].join('\n')
}

function record(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

function text(value, max) {
  const result = String(value ?? '').trim()
  return result && result.length <= max ? result : ''
}

// 解析决定：只认 JSON 里的 actions；不合法的条目丢掉，整段解析不出来就当没有决定（回复照常）。
export function parseBotDecisions(raw) {
  const source = String(raw || '')
  const start = source.indexOf('{')
  const end = source.lastIndexOf('}')
  if (start < 0 || end <= start) return []
  let parsed
  try {
    parsed = JSON.parse(source.slice(start, end + 1))
  } catch {
    return []
  }
  const actions = Array.isArray(record(parsed).actions) ? parsed.actions : []
  const decisions = []
  for (const item of actions.slice(0, MAX_DECISIONS)) {
    const action = record(item)
    if (action.type === 'spawn') {
      const title = text(action.title, MAX_TITLE_CHARS)
      const task = text(action.task, MAX_TASK_CHARS)
      if (!title || !task) continue
      const model =
        typeof action.model === 'string' && action.model.includes('/') ? action.model.trim() : ''
      const tier = TIERS.has(String(action.tier)) ? String(action.tier) : ''
      decisions.push({
        type: 'spawn',
        title,
        task,
        ...(model ? { model } : {}),
        ...(tier ? { tier } : {}),
      })
    } else if (action.type === 'add') {
      const agent = text(action.agent, 64)
      const addText = text(action.text, MAX_ADD_CHARS)
      if (agent && addText) decisions.push({ type: 'add', agent, text: addText })
    } else if (action.type === 'stop') {
      const agent = text(action.agent, 64)
      if (agent) decisions.push({ type: 'stop', agent })
    } else if (action.type === 'remember') {
      const memory = text(action.text, MAX_MEMORY_CHARS)
      const source = MEMORY_SOURCES.has(String(action.source)) ? String(action.source) : 'told'
      if (memory) decisions.push({ type: 'remember', text: memory, source })
    } else if (action.type === 'forget') {
      const memory = text(action.memory, 64)
      if (memory) decisions.push({ type: 'forget', memory })
    }
  }
  return decisions
}

// 决定里的子 agent 引用（id 前缀）→ 这个 Bot 自己的子 agent；对不上或有歧义时为空，不碰别人的。
export function resolveBotAgent(agents, reference) {
  const prefix = String(reference || '').trim()
  if (!prefix) return null
  const matches = agents.filter((agent) => agent.id.startsWith(prefix))
  return matches.length === 1 ? matches[0] : null
}
