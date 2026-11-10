// 子代理过程的只读投影：把子代理会话里的 Pi 消息投影成与普通会话相同的界面消息（含工具活动），
// 供检查器的“子代理”标签显示。主代理交代的第一条任务单独给出，界面放在顶部的“任务”块里。
import { serializeTranscriptMessages, textFromContent } from './stream-projection.mjs'

// 一次最多给最近这么多条界面消息；更早的只报条数（子代理很少超过）。
export const AGENT_TRANSCRIPT_MESSAGE_LIMIT = 200
export const AGENT_TASK_TEXT_LIMIT = 12_000

/**
 * @param {{ agent: Record<string, unknown>, active: boolean, messages: unknown[] | null } | null} source
 * @param {{ limit?: number }} [options]
 */
export function projectAgentTranscript(source, { limit = AGENT_TRANSCRIPT_MESSAGE_LIMIT } = {}) {
  if (!source) return null
  const { agent, active } = source
  // 这个功能之前派出的子代理只在内存里跑过，没有留下记录。
  if (!Array.isArray(source.messages)) return { agent, active, available: false }
  const raw = source.messages
  const first = raw.find((message) => message?.role === 'user')
  const task = String(first ? textFromContent(first.content) : agent.message || '').slice(
    0,
    AGENT_TASK_TEXT_LIMIT,
  )
  let messages = serializeTranscriptMessages(raw)
  // 还在跑时，最后一步里还没有结果的工具是“正在执行”，不是投影默认写的“没有记录到结果”。
  if (active && messages.length) {
    const resolved = new Set(
      raw.filter((message) => message?.role === 'toolResult').map((message) => message.toolCallId),
    )
    const last = messages.at(-1)
    const activity = last?.runActivity
    if (activity) {
      const running = (tool) =>
        tool?.type === 'tool' && !resolved.has(tool.id)
          ? { ...tool, status: 'running', message: '', finishedAt: null }
          : tool
      messages = [
        ...messages.slice(0, -1),
        {
          ...last,
          runActivity: {
            ...activity,
            tools: activity.tools.map(running),
            activityFeed: activity.activityFeed.map(running),
            finishedAt: null,
            // 这一轮还没结束：工具组标题写“运行中”，还在跑的工具显示计时，而不是“全部完成”。
            streaming: true,
          },
        },
      ]
    }
  }
  const max = Math.max(1, Number(limit) || AGENT_TRANSCRIPT_MESSAGE_LIMIT)
  const omitted = Math.max(0, messages.length - max)
  return {
    agent,
    active,
    available: true,
    task,
    // 第一条是任务本身时界面会隐藏它（已经在“任务”块里）；被截掉时就不再是第一条。
    taskShown: omitted === 0 && messages[0]?.role === 'user',
    omitted,
    messages: omitted ? messages.slice(-max) : messages,
  }
}
