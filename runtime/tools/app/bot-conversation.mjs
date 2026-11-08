// Bot 对话专用工具（v2.1）：对话自己能读、查、小改、跑短命令，长的、要批准的交给子 agent；这些工具是它当经理用的手：
// 派活 / 补充 / 停、问用户（问题卡片，发出就结束等待，回答作为下一条消息回来）、给自己定时、记住 / 忘掉 / 回忆、
// 找别的 Bot。都不阻塞：派出去的活、问题、定时都立即返回。只装在 Bot 的对话上（不是子 agent、不是普通会话），
// 真正的执行（工具范围、记忆、技能、跳数与频率限制、“这一轮只出建议”）在运行时里校验。
import { Type } from 'typebox'
import { defineTool } from '../../runtime/pi-coding-agent.mjs'
import { ASK_USER_QUESTION_SCHEMA } from './ask-user.mjs'
import { isoDate, memoryLine } from './memory.mjs'
import {
  BOT_ASK,
  BOT_FORGET,
  BOT_JOB_ADD,
  BOT_JOB_START,
  BOT_JOB_STOP,
  BOT_MESSAGE,
  BOT_NOTIFY,
  BOT_RECALL,
  BOT_REMEMBER,
  BOT_SCHEDULE,
} from '../../prompts/tools/bot-conversation.mjs'

export { BOT_CONVERSATION_TOOL_NAMES } from './bot-conversation-tool-names.mjs'

function text(result) {
  return {
    content: [{ type: 'text', text: String(result?.text || result || 'Done.') }],
    details: result,
  }
}

const TIER = Type.String({ enum: ['fast', 'standard', 'strong'] })

/** @param {{ runtime: Record<string, (input: any) => Promise<any>> }} options */
export function createBotConversationTools({ runtime }) {
  const call = (name) => async (_toolCallId, params) => text(await runtime[name](params || {}))
  return [
    defineTool({
      name: 'bot_job_start',
      label: 'Start a job',
      description: BOT_JOB_START.description,
      promptSnippet: BOT_JOB_START.snippet,
      parameters: Type.Object({
        title: Type.String({
          minLength: 1,
          maxLength: 80,
          description: BOT_JOB_START.params.title,
        }),
        task: Type.String({
          minLength: 1,
          maxLength: 8000,
          description: BOT_JOB_START.params.task,
        }),
        model: Type.Optional(Type.String({ description: BOT_JOB_START.params.model })),
        tier: Type.Optional(TIER),
      }),
      execute: call('startJob'),
    }),
    defineTool({
      name: 'bot_job_add',
      label: 'Add to a job',
      description: BOT_JOB_ADD.description,
      parameters: Type.Object({
        job: Type.String({ minLength: 1, maxLength: 64, description: BOT_JOB_ADD.params.job }),
        text: Type.String({ minLength: 1, maxLength: 4000 }),
      }),
      execute: call('addToJob'),
    }),
    defineTool({
      name: 'bot_job_stop',
      label: 'Stop a job',
      description: BOT_JOB_STOP.description,
      parameters: Type.Object({
        job: Type.String({ minLength: 1, maxLength: 64 }),
      }),
      execute: call('stopJob'),
    }),
    defineTool({
      name: 'bot_ask',
      label: 'Ask the user',
      description: BOT_ASK.description,
      parameters: Type.Object({
        questions: Type.Array(ASK_USER_QUESTION_SCHEMA, { minItems: 1, maxItems: 4 }),
      }),
      execute: call('ask'),
    }),
    defineTool({
      name: 'bot_schedule',
      label: 'Schedule a message to yourself',
      description: BOT_SCHEDULE.description,
      parameters: Type.Object({
        action: Type.String({ enum: ['create', 'list', 'remove'] }),
        message: Type.Optional(
          Type.String({
            maxLength: 4000,
            description: BOT_SCHEDULE.params.message,
          }),
        ),
        inMinutes: Type.Optional(
          Type.Integer({
            minimum: 1,
            maximum: 60 * 24 * 30,
            description: BOT_SCHEDULE.params.inMinutes,
          }),
        ),
        date: Type.Optional(
          Type.String({
            pattern: '^\\d{4}-\\d{2}-\\d{2}$',
            description: BOT_SCHEDULE.params.date,
          }),
        ),
        time: Type.Optional(
          Type.String({
            pattern: '^([01]\\d|2[0-3]):[0-5]\\d$',
            description: BOT_SCHEDULE.params.time,
          }),
        ),
        repeat: Type.Optional(
          Type.String({
            enum: ['daily', 'weekly'],
            description: BOT_SCHEDULE.params.repeat,
          }),
        ),
        weekday: Type.Optional(
          Type.Integer({ minimum: 0, maximum: 6, description: BOT_SCHEDULE.params.weekday }),
        ),
        id: Type.Optional(Type.String({ maxLength: 80, description: BOT_SCHEDULE.params.id })),
      }),
      execute: call('schedule'),
    }),
    defineTool({
      name: 'bot_remember',
      label: 'Remember',
      description: BOT_REMEMBER.description,
      parameters: Type.Object({
        text: Type.String({ minLength: 1, maxLength: 500 }),
        source: Type.Optional(Type.String({ enum: ['told', 'correction', 'review', 'proactive'] })),
        again: Type.Optional(
          Type.Boolean({
            description: BOT_REMEMBER.params.again,
          }),
        ),
      }),
      execute: call('remember'),
    }),
    defineTool({
      name: 'bot_forget',
      label: 'Forget',
      description: BOT_FORGET.description,
      parameters: Type.Object({ memory: Type.String({ minLength: 1, maxLength: 64 }) }),
      execute: call('forget'),
    }),
    defineTool({
      name: 'bot_recall',
      label: 'Recall',
      description: BOT_RECALL.description,
      promptSnippet: BOT_RECALL.snippet,
      parameters: Type.Object({
        query: Type.String({ minLength: 1, maxLength: 2000 }),
        queries: Type.Optional(
          Type.Array(Type.String({ minLength: 1, maxLength: 400 }), {
            maxItems: 3,
            description: BOT_RECALL.params.queries,
          }),
        ),
        since: Type.Optional(Type.String({ description: BOT_RECALL.params.since })),
        until: Type.Optional(Type.String({ description: BOT_RECALL.params.until })),
        include_history: Type.Optional(
          Type.Boolean({ description: BOT_RECALL.params.include_history }),
        ),
        order: Type.Optional(
          Type.String({
            enum: ['relevance', 'recent', 'chronological'],
            description: BOT_RECALL.params.order,
          }),
        ),
        level: Type.Optional(
          Type.String({
            enum: ['facts', 'digests'],
            description: BOT_RECALL.params.level,
          }),
        ),
      }),
      execute: async (_toolCallId, params = {}) => {
        const result = await runtime.recall({
          query: params.query,
          queries: params.queries || [],
          since: isoDate(params.since),
          until: isoDate(params.until),
          includeHistory: params.include_history,
          order: params.order,
          level: params.level,
        })
        return text({
          ...result,
          text: result.memories.length
            ? result.memories.map(memoryLine).join('\n\n')
            : 'No related memories found.',
        })
      },
    }),
    defineTool({
      name: 'bot_message',
      label: 'Message another bot',
      description: BOT_MESSAGE.description,
      parameters: Type.Object({
        bot: Type.String({ minLength: 1, maxLength: 64, description: BOT_MESSAGE.params.bot }),
        text: Type.String({ minLength: 1, maxLength: 4000 }),
        expectsReply: Type.Optional(Type.Boolean()),
      }),
      execute: call('message'),
    }),
    defineTool({
      name: 'bot_notify',
      label: 'Notify the user on their chat apps',
      description: BOT_NOTIFY.description,
      parameters: Type.Object({
        text: Type.String({ minLength: 1, maxLength: 2000 }),
        to: Type.Optional(
          Type.String({
            enum: ['me', 'group'],
            description: BOT_NOTIFY.params.to,
          }),
        ),
        group: Type.Optional(
          Type.String({
            maxLength: 200,
            description: BOT_NOTIFY.params.group,
          }),
        ),
        platform: Type.Optional(
          Type.String({
            enum: ['qq', 'telegram', 'feishu', 'weixin'],
            description: BOT_NOTIFY.params.platform,
          }),
        ),
      }),
      execute: call('notify'),
    }),
  ]
}
