import { extractConversationMemories } from '../services/memory/conversation-memory.mjs'

export function localDayKey(value = new Date()) {
  const date = value instanceof Date ? value : new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  const pad = (part) => String(part).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/**
 * 从一轮对话中抽取候选记忆（对话提取由模型完成，结果仍按置信度走
 * propose 的自动确认/待审核流程）。
 */
export async function captureConversationMemory(
  runtime,
  { sessionId, cwd, model, user, assistant, sourceTimestamp = '' },
) {
  const result = await extractConversationMemories({
    modelRuntime: runtime.modelRuntime,
    model,
    user,
    assistant,
  })
  if (result.usage)
    await runtime.recordUsage(
      localDayKey(result.timestamp || Date.now()),
      `memory:${sessionId}:${result.timestamp || Date.now()}`,
      result.usage,
    )
  if (!result.memories.length) return []
  const projectSpaceId = await runtime.memory.ensureWorkspaceSpace(cwd)
  return result.memories.map((item, index) =>
    runtime.memory.propose({
      ...item,
      spaceId: item.scope === 'global' ? 'global' : projectSpaceId,
      cwd,
      sessionId,
      sourceId: `${sessionId}:${sourceTimestamp || result.timestamp || Date.now()}:${index}`,
      sourceTimestamp: sourceTimestamp || new Date(result.timestamp || Date.now()).toISOString(),
      sourceType: 'conversation',
    }),
  )
}
