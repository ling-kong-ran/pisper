// 手机会话页的“上一轮摘要胶囊”（规格 7.3）：转录只展开最新一轮，之前的对话收成一枚胶囊，
// 胶囊写上一轮的轮次、提问首行与操作数，点开再显示完整历史。纯函数，只读消息，不改任何状态。
import type { ChatMessage, EntityRecord } from '@/types/chat'

export type EarlierTurnSummary = {
  // 最新一轮（最后一条用户消息）在消息列表里的下标；收起时从这里开始渲染。
  latestIndex: number
  // 被收起的用户消息条数，也是最新一轮之前已经过去的轮数（用于“第 N 轮”继续编号）。
  turnBase: number
  // 上一轮：轮次（历史没有从开头加载时数不准，为 null）、提问首行、工具操作数与是否以失败结束。
  previous: { turn: number | null; text: string; operations: number; failed: boolean }
}

// 一轮里的工具操作数，与工具组标题“N 个操作”同一口径（活动流优先，没有时用工具列表）。
function operationCount(message: ChatMessage) {
  const activity = message.runActivity as EntityRecord | null | undefined
  const feed: unknown = activity?.activityFeed
  const tools: unknown = activity?.tools
  if (Array.isArray(feed) && feed.length) return feed.length
  return Array.isArray(tools) ? tools.length : 0
}

function firstLine(text: unknown) {
  return (
    String(text || '')
      .split('\n')
      .map((line) => line.trim())
      .find(Boolean) || ''
  )
}

export function earlierTurnSummary(
  messages: readonly ChatMessage[],
  { turnsKnown }: { turnsKnown: boolean },
): EarlierTurnSummary | null {
  let latestIndex = -1
  for (let index = messages.length - 1; index >= 0; index -= 1)
    if (messages[index]?.role === 'user') {
      latestIndex = index
      break
    }
  // 只有一轮（或最新一轮之前没有任何消息）时不收起。
  if (latestIndex <= 0) return null
  let previousIndex = -1
  let turnBase = 0
  for (let index = 0; index < latestIndex; index += 1)
    if (messages[index]?.role === 'user') {
      previousIndex = index
      turnBase += 1
    }
  let operations = 0
  let failed = false
  for (let index = Math.max(0, previousIndex); index < latestIndex; index += 1) {
    const message = messages[index]
    if (message?.role !== 'agent') continue
    operations += operationCount(message)
    failed = Boolean(message.error)
  }
  return {
    latestIndex,
    turnBase,
    previous: {
      turn: turnsKnown && previousIndex >= 0 ? turnBase : null,
      text: previousIndex >= 0 ? firstLine(messages[previousIndex]?.text) : '',
      operations,
      failed,
    },
  }
}
