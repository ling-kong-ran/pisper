// Bot 对话专用工具（bot_*）的说明。对话当经理：派活、问用户、给自己定时、记忆、找别的 Bot、发到聊天应用。

export const BOT_JOB_START = Object.freeze({
  description:
    'Hand a job to a background sub-agent: anything long, multi-step, or that needs the user’s approval. It runs in parallel with your tools and its own approvals; you are told when it finishes. Returns immediately with the job id.',
  snippet: 'Start a background sub-agent job for long or approval-needing work',
  params: {
    title: 'Short title in the user’s language.',
    task: 'Self-contained instructions: goal, relevant paths and context, what to report back. Name a skill from [Your skills] when one fits.',
    model: 'provider/model from the available list (optional).',
  },
})

export const BOT_JOB_ADD = Object.freeze({
  description:
    'Send a follow-up to a running job (delivered after its current step, without interrupting it).',
  params: { job: 'Job id prefix.' },
})

export const BOT_JOB_STOP = Object.freeze({
  description: 'Stop a running job or background command (id prefix from [Current state]).',
})

export const BOT_ASK = Object.freeze({
  description:
    'Ask the user 1–4 multiple-choice questions as a card. Use it only when the user must choose and you cannot reasonably decide yourself; never to confirm a plan you can carry out. It does not wait: end your turn after asking; the answers arrive as the next message.',
})

export const BOT_SCHEDULE = Object.freeze({
  description:
    'Schedule a message to yourself, for anything that should happen later or repeatedly ("in an hour, check CI"; "every Friday, follow skill X"). At that time the message arrives in this conversation and you act on it. Never start a job that just waits. Also lists and removes your own schedules.',
  params: {
    message: 'What you want to be told / asked to do at that time.',
    inMinutes: 'Once, this many minutes from now.',
    date: 'Once, on this date (YYYY-MM-DD).',
    time: 'HH:MM, local time.',
    repeat: 'Repeat daily or weekly at time instead of once.',
    weekday: '0 = Sunday.',
    id: 'Schedule id, for remove.',
  },
})

export const BOT_REMEMBER = Object.freeze({
  description:
    'Remember one short durable fact, preference or rule (what the user asked you to remember, a correction such as "don’t … again", a lesson from finished work). Not for one-off task details.',
  params: { again: 'Only when the user just asked again to remember something they had deleted.' },
})

export const BOT_FORGET = Object.freeze({
  description:
    'Forget a memory the user says is wrong or no longer true. Use its id from <pisper_memory_context> or bot_recall; the first 6 characters after mem_ are enough.',
})

export const BOT_RECALL = Object.freeze({
  description:
    'Search your memory. What relates to the current message is already attached to it; use this for anything else: older things, what happened in a period, how something changed. Pass the question and rewordings, including the other language (Chinese and English), because search matches words.',
  snippet: 'Search your own memory beyond what is attached to this message',
  params: {
    queries: 'Rewordings of the query, for example in the other language.',
    since: 'Only results from this ISO date on.',
    until: 'Only results before this ISO date.',
    include_history: 'Also return facts that a newer version has replaced.',
    order: 'relevance (default), recent first, or oldest first.',
    level: 'facts (default), or summaries by day, week and month first.',
  },
})

export const BOT_MESSAGE = Object.freeze({
  description:
    'Ask another bot for something that is its responsibility. Its reply, and the result of work it does for you, come back here automatically. Do not repeat a reply that is forwarded automatically, and never send courtesy messages.',
  params: { bot: 'The other bot’s name.' },
})

export const BOT_NOTIFY = Object.freeze({
  description:
    'Send a short message on the chat apps connected to Pisper (QQ, Telegram, Feishu, WeChat). By default it goes to the user\'s own private chat with the Pisper bot; with to="group" it is posted in one of the groups in your chat rules, where everyone in the group sees it, so never put private details there. Use it when asked to tell someone there, typically from a scheduled check ("when it passes, tell me on QQ", "post the daily summary in the team group"). Your normal reply only stays in this conversation; if you are being talked to from a chat right now, your reply already goes to that chat, so do not also send it. A schedule made from a group names that group\'s ID in its message.',
  params: {
    to: "me (default): the user's private chats. group: a group in your chat rules.",
    group:
      'For to="group": the group ID or name. Omit to use the group this message came from, or your only group.',
    platform: 'Only this app. Omit to send to every connected app (or any group).',
  },
})
