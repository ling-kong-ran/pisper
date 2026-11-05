// Bot 的对话（v2.1）：对话自己做快的事（读、查、小改、短命令），长的、多步的、要批准的交给子 agent 在后台并行跑；
// 运行时保证它不阻塞（新消息插进来、长命令转后台、不在对话里等批准）。它当经理用的手是 bot_* 工具。
// 这里是纯函数：对话的系统提示、每轮附带的状态快照、bot_* 工具输入的归一。运行时接线在 agent-runtime-facade。

export const BOT_MAX_PARALLEL_AGENTS = 3
const MAX_TITLE_CHARS = 80
const MAX_TASK_CHARS = 8_000
const MAX_ADD_CHARS = 4_000
const TIERS = new Set(['fast', 'standard', 'strong'])
const MEMORY_SOURCES = new Set(['told', 'correction', 'review', 'proactive'])
const MAX_MEMORY_CHARS = 500
const MAX_MESSAGE_CHARS = 4_000
const SNAPSHOT_OUTPUT_CHARS = 300

// 对话的系统提示：它是谁、它负责什么、它怎么干活（快的自己做，长的交给子 agent，不为确认而停）。
export function botConversationPrompt(bot) {
  const parts = [
    `你是 ${bot.name}，用户在 Pisper 里的一个长期助手。这是你和用户的一段持续对话：前文都算数，消息也可能来自用户的聊天软件、定时任务或别的 Bot。`,
    [
      '怎么干活：',
      '- 快的事自己做：读文件、搜索、小改动、跑短命令、查资料。命令跑超过 10 秒会自动转到后台，结果之后作为一条消息回来，不用等它，也不要轮询。',
      '- 长的、多步的、要用户批准的事，用 bot_job_start 交给子 agent 在后台做（可以同时做好几件），bot_job_add 补充、bot_job_stop 停。要批准的操作你自己做会被退回，直接交给子 agent。',
      '- 以后再做、到点做或要重复做的事，用 bot_schedule 给自己定时；不要派一个干等的子 agent。',
      '- 只有用户必须拍板、你确实没法自己判断时，才用 bot_ask 发问题卡片，发完就结束这一轮，回答会作为下一条消息回来。不要为了确认计划而停下来问：能做的直接做，做完说一声。',
      '- 用户让你记住的、纠正你的（“以后别…”）、做完学到的，用 bot_remember，记下就生效，不用用户确认；说错了的用 bot_forget。',
      '- 记忆不会全部摆在面前：和这句话相关的会附在消息后面（<pisper_memory_context>，从旧到新，矛盾时信新的），想查更多（以前的事、某段时间的事、某件事怎么变过）用 bot_recall。',
      '- 别的 Bot 负责的事，用 bot_message 找它；它的回话和它做完的结果会自动回到这里。别的 Bot 发来的消息会写明来自谁。',
      '- 用户随时可能发来新消息打断你：先回应新消息，再接着做没做完的事。',
      '- 子 agent 做完、后台命令跑完，结果会作为一条消息回到这里：用一两句话汇报结论，不要整段照抄。',
      '- 每条消息后面附有 [当前状态]：手上的活、后台命令、可用的模型与别的 Bot、你会的技能。',
      '- 不要说你做了没做的事，也不要承诺系统没有的东西（比如“会弹确认框”）；做不到就直说。',
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

const ACTIVE = new Set(['queued', 'starting', 'running'])

// 每轮附带的状态快照（不显示给人看）：手上的子 agent 与后台命令、各自状态与最近进展、在等几个批准、
// 可以挑的模型、可以找的别的 Bot。
export function botStateSnapshot({
  agents = [],
  commands = [],
  waitingApprovals = 0,
  limit = BOT_MAX_PARALLEL_AGENTS,
  models = [],
  bots = [],
} = {}) {
  const active = agents.filter((agent) => ACTIVE.has(agent.status))
  const recent = agents.filter((agent) => !ACTIVE.has(agent.status)).slice(-5)
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
  const running = commands.filter((command) => command.status === 'running')
  if (running.length)
    rows.push(
      '后台命令：',
      ...running.map(
        (command) => `- [${command.id}] ${clip(command.command || command.title, 120)}`,
      ),
    )
  if (models.length) rows.push(`可以挑的模型（bot_job_start 的 model）：${models.join(', ')}`)
  if (bots.length)
    rows.push(
      `可以找的别的 Bot：${bots.map((bot) => `${bot.name}${bot.instructions ? `（${clip(bot.instructions, 80)}）` : ''}`).join('；')}`,
    )
  return rows.join('\n')
}

function record(value) {
  return value && typeof value === 'object' && !Array.isArray(value) ? value : {}
}

function text(value, max) {
  const result = String(value ?? '').trim()
  return result && result.length <= max ? result : ''
}

// bot_* 工具的输入 → 运行时要执行的动作；不合法时为 null（工具据此报错，不执行）。
export function normalizeBotAction(input) {
  const action = record(input)
  if (action.type === 'spawn') {
    const title = text(action.title, MAX_TITLE_CHARS)
    const task = text(action.task, MAX_TASK_CHARS)
    if (!title || !task) return null
    const model =
      typeof action.model === 'string' && action.model.includes('/') ? action.model.trim() : ''
    const tier = TIERS.has(String(action.tier)) ? String(action.tier) : ''
    return { type: 'spawn', title, task, ...(model ? { model } : {}), ...(tier ? { tier } : {}) }
  }
  if (action.type === 'add') {
    const agent = text(action.agent, 64)
    const addText = text(action.text, MAX_ADD_CHARS)
    return agent && addText ? { type: 'add', agent, text: addText } : null
  }
  if (action.type === 'stop') {
    const agent = text(action.agent, 64)
    return agent ? { type: 'stop', agent } : null
  }
  if (action.type === 'remember') {
    const memory = text(action.text, MAX_MEMORY_CHARS)
    const source = MEMORY_SOURCES.has(String(action.source)) ? String(action.source) : 'told'
    if (!memory) return null
    return {
      type: 'remember',
      text: memory,
      source,
      ...(action.again === true ? { again: true } : {}),
    }
  }
  if (action.type === 'forget') {
    const memory = text(action.memory, 64)
    return memory ? { type: 'forget', memory } : null
  }
  if (action.type === 'message') {
    const bot = text(action.bot, 64)
    const messageText = text(action.text, MAX_MESSAGE_CHARS)
    return bot && messageText
      ? { type: 'message', bot, text: messageText, expectsReply: action.expectsReply !== false }
      : null
  }
  return null
}

// 决定里的子 agent 引用（id 前缀）→ 这个 Bot 自己的子 agent；对不上或有歧义时为空，不碰别人的。
export function resolveBotAgent(agents, reference) {
  const prefix = String(reference || '').trim()
  if (!prefix) return null
  const matches = agents.filter((agent) => agent.id.startsWith(prefix))
  return matches.length === 1 ? matches[0] : null
}

const MAX_REVIEW_OUTPUT_CHARS = 6_000

// 做完一件活后的复盘：这类活会不会反复做？会就写成（或更新）一个技能。只要 JSON。
export function botReviewInstruction({
  title = '',
  task = '',
  output = '',
  skills = [],
  corrections = [],
} = {}) {
  return [
    'You are reviewing a job your sub-agent just finished, to decide whether to save how it was done as a reusable skill.',
    'Write a skill only when this kind of work is likely to come up again (a repeatable procedure: a release check, a kind of bug fix, a report format), not for a one-off.',
    "If an existing skill already covers it, update that skill (same name) only when this run or the user's corrections taught something new; otherwise return no skill.",
    "A skill body is Markdown instructions for a future sub-agent: when to use it, the steps, commands and paths that worked, pitfalls, what to report. Keep it short and concrete. Write it in the user's language.",
    'Respond with JSON only: {"skill": null} or {"skill": {"name": "<lowercase-kebab-case>", "description": "<what it does and when to use it>", "body": "<markdown>"}}.',
    '',
    `Job: ${clip(title, MAX_TITLE_CHARS)}`,
    `Instructions given: ${clip(task, 2_000)}`,
    `Result: ${String(output || '').slice(0, MAX_REVIEW_OUTPUT_CHARS)}`,
    '',
    `Existing skills: ${skills.length ? skills.map((skill) => `${skill.name} (${clip(skill.description, 160)})`).join('; ') : 'none'}.`,
    ...(corrections.length
      ? ['User corrections to respect:', ...corrections.map((item) => `- ${clip(item, 300)}`)]
      : []),
  ].join('\n')
}

// 解析复盘结果：{"skill": {...}} 才算要写；格式不对、名字不合法、正文为空时当没有（复盘失败不影响别的）。
export function parseBotSkillReview(raw) {
  const source = String(raw || '')
  const start = source.indexOf('{')
  const end = source.lastIndexOf('}')
  if (start < 0 || end <= start) return null
  let parsed
  try {
    parsed = JSON.parse(source.slice(start, end + 1))
  } catch {
    return null
  }
  const skill = record(record(parsed).skill)
  const name = String(skill.name || '').trim()
  const description = String(skill.description || '').trim()
  const body = String(skill.body || '').trim()
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name) || name.length > 64) return null
  if (!description || !body) return null
  return { name, description: description.slice(0, 1_024), body: body.slice(0, 20_000) }
}
