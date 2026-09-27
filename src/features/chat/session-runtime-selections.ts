import { chatApi } from './chat-api'
import { createRuntimeSelectionQueue } from './runtime-selection-queue'

// Lifetime belongs to queued commands, not the chat view. Leaving for settings or
// workflows must neither cancel a selection nor keep a React tree mounted.
export const sessionRuntimeSelections = createRuntimeSelectionQueue({
  isStreaming: async (id, signal) =>
    Boolean((await chatApi.getLiveSession(id, { signal })).streaming),
  apply: (id, selection) =>
    selection.model
      ? chatApi.updateModel(id, selection.model.provider, selection.model.modelId)
      : chatApi.setThinkingLevel(id, selection.thinkingLevel!),
})
