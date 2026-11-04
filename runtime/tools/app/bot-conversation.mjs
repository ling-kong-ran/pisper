// Bot 对话专用工具（v2.1）：对话自己能读、查、小改、跑短命令，长的、要批准的交给子 agent；这些工具是它当经理用的手：
// 派活 / 补充 / 停、问用户（问题卡片，发出就结束等待，回答作为下一条消息回来）、给自己定时、记住 / 忘掉、
// 找别的 Bot。都不阻塞：派出去的活、问题、定时都立即返回。只装在 Bot 的对话上（不是子 agent、不是普通会话），
// 真正的执行（工具范围、记忆、技能、跳数与频率限制、“这一轮只出建议”）在运行时里校验。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { ASK_USER_QUESTION_SCHEMA } from './ask-user.mjs'

export { BOT_CONVERSATION_TOOL_NAMES } from './bot-conversation-tool-names.mjs'

function text(result) {
  return {
    content: [{ type: 'text', text: String(result?.text || result || 'Done.') }],
    details: result,
  }
}

const TIER = Type.Union(['fast', 'standard', 'strong'].map((value) => Type.Literal(value)))

/** @param {{ runtime: Record<string, (input: any) => Promise<any>> }} options */
export function createBotConversationTools({ runtime }) {
  const call = (name) => async (_toolCallId, params) => text(await runtime[name](params || {}))
  return [
    defineTool({
      name: 'bot_job_start',
      label: 'Start a job',
      description:
        'Hand a job to a background sub-agent: anything long, multi-step, or that needs the user’s approval. It runs in parallel with your tools and its own approvals; you are told when it finishes. Returns immediately with the job id.',
      promptSnippet: 'Start a background sub-agent job for long or approval-needing work',
      parameters: Type.Object({
        title: Type.String({
          minLength: 1,
          maxLength: 80,
          description: 'Short title in the user’s language.',
        }),
        task: Type.String({
          minLength: 1,
          maxLength: 8000,
          description:
            'Self-contained instructions: goal, relevant paths and context, what to report back. Name a skill from [你会的技能] when one fits.',
        }),
        model: Type.Optional(
          Type.String({ description: 'provider/model from the available list (optional).' }),
        ),
        tier: Type.Optional(TIER),
      }),
      execute: call('startJob'),
    }),
    defineTool({
      name: 'bot_job_add',
      label: 'Add to a job',
      description:
        'Send a follow-up to a running job (delivered after its current step, without interrupting it).',
      parameters: Type.Object({
        job: Type.String({ minLength: 1, maxLength: 64, description: 'Job id prefix.' }),
        text: Type.String({ minLength: 1, maxLength: 4000 }),
      }),
      execute: call('addToJob'),
    }),
    defineTool({
      name: 'bot_job_stop',
      label: 'Stop a job',
      description: 'Stop a running job or background command (id prefix from [当前状态]).',
      parameters: Type.Object({
        job: Type.String({ minLength: 1, maxLength: 64 }),
      }),
      execute: call('stopJob'),
    }),
    defineTool({
      name: 'bot_ask',
      label: 'Ask the user',
      description:
        'Ask the user 1–4 multiple-choice questions as a card. Use it only when the user must choose and you cannot reasonably decide yourself; never to confirm a plan you can carry out. It does not wait: end your turn after asking; the answers arrive as the next message.',
      parameters: Type.Object({
        questions: Type.Array(ASK_USER_QUESTION_SCHEMA, { minItems: 1, maxItems: 4 }),
      }),
      execute: call('ask'),
    }),
    defineTool({
      name: 'bot_schedule',
      label: 'Schedule a message to yourself',
      description:
        'Schedule a message to yourself, for anything that should happen later or repeatedly ("in an hour, check CI"; "every Friday, follow skill X"). At that time the message arrives in this conversation and you act on it. Never start a job that just waits. Also lists and removes your own schedules.',
      parameters: Type.Object({
        action: Type.Union([Type.Literal('create'), Type.Literal('list'), Type.Literal('remove')]),
        message: Type.Optional(
          Type.String({
            maxLength: 4000,
            description: 'What you want to be told / asked to do at that time.',
          }),
        ),
        inMinutes: Type.Optional(
          Type.Integer({
            minimum: 1,
            maximum: 60 * 24 * 30,
            description: 'Once, this many minutes from now.',
          }),
        ),
        date: Type.Optional(
          Type.String({
            pattern: '^\\d{4}-\\d{2}-\\d{2}$',
            description: 'Once, on this date (YYYY-MM-DD).',
          }),
        ),
        time: Type.Optional(
          Type.String({
            pattern: '^([01]\\d|2[0-3]):[0-5]\\d$',
            description: 'HH:MM, local time.',
          }),
        ),
        repeat: Type.Optional(
          Type.Union([Type.Literal('daily'), Type.Literal('weekly')], {
            description: 'Repeat daily or weekly at time instead of once.',
          }),
        ),
        weekday: Type.Optional(
          Type.Integer({ minimum: 0, maximum: 6, description: '0 = Sunday.' }),
        ),
        id: Type.Optional(Type.String({ maxLength: 80, description: 'Schedule id, for remove.' })),
      }),
      execute: call('schedule'),
    }),
    defineTool({
      name: 'bot_remember',
      label: 'Remember',
      description:
        'Remember one short durable fact, preference or rule (what the user asked you to remember, a correction such as "以后别…", a lesson from finished work). Not for one-off task details.',
      parameters: Type.Object({
        text: Type.String({ minLength: 1, maxLength: 500 }),
        source: Type.Optional(
          Type.Union(
            ['told', 'correction', 'review', 'proactive'].map((value) => Type.Literal(value)),
          ),
        ),
      }),
      execute: call('remember'),
    }),
    defineTool({
      name: 'bot_forget',
      label: 'Forget',
      description:
        'Forget a memory the user says is wrong or no longer true (short id from [你记得的事]).',
      parameters: Type.Object({ memory: Type.String({ minLength: 1, maxLength: 64 }) }),
      execute: call('forget'),
    }),
    defineTool({
      name: 'bot_message',
      label: 'Message another bot',
      description:
        'Ask another bot for something that is its responsibility. Its reply, and the result of work it does for you, come back here automatically. Do not repeat a reply that is forwarded automatically, and never send courtesy messages.',
      parameters: Type.Object({
        bot: Type.String({ minLength: 1, maxLength: 64, description: 'The other bot’s name.' }),
        text: Type.String({ minLength: 1, maxLength: 4000 }),
        expectsReply: Type.Optional(Type.Boolean()),
      }),
      execute: call('message'),
    }),
  ]
}
