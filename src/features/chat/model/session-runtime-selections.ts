import { chatApi } from '@/features/chat/api/chat-api'
import { createRuntimeSelectionQueue } from '@/features/chat/model/runtime-selection-queue'

// Lifetime belongs to queued commands, not the chat view. Leaving for settings or
// workflows must neither cancel a selection nor keep a React tree mounted.
export const sessionRuntimeSelections = createRuntimeSelectionQueue({
  isStreaming: async (id, signal) => {
    const snapshot = await chatApi.getLiveSession(id, { signal })
    // 旧 Runtime 只有 streaming；新 Runtime 在停止显示后继续报告配置写入是否仍被占用。
    return Boolean(snapshot.configurationBusy ?? snapshot.streaming)
  },
  apply: (id, selection) =>
    selection.model
      ? chatApi.updateModel(id, selection.model.provider, selection.model.modelId)
      : chatApi.setThinkingLevel(id, selection.thinkingLevel!),
})
