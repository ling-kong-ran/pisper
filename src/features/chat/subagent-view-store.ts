// 检查器“子代理”标签当前看的是哪个子代理：按会话记，只在内存里（刷新后标签不再出现，与侧聊一样按需打开）。
import { create } from 'zustand'

type SubagentViewState = {
  selected: Record<string, string>
  select: (sessionId: string, agentId: string) => void
  close: (sessionId: string) => void
}

export const useSubagentViewStore = create<SubagentViewState>((set) => ({
  selected: {},
  select: (sessionId, agentId) =>
    set((state) =>
      state.selected[sessionId] === agentId
        ? state
        : { selected: { ...state.selected, [sessionId]: agentId } },
    ),
  close: (sessionId) =>
    set((state) => {
      if (!(sessionId in state.selected)) return state
      const { [sessionId]: _closed, ...selected } = state.selected
      return { selected }
    }),
}))
