// 从看板打开会话前记下的位置：各泳道滚动位置与来源卡片。返回看板（⌘[、返回键、标签）时
// 页面重新挂载，据此恢复滚动并把焦点还给来源卡片。只保存在内存里，刷新页面后不恢复。
export type BoardReturnState = {
  focusId: string
  scroll: Record<string, number>
}

let pending: BoardReturnState | null = null

export function rememberBoardReturn(state: BoardReturnState) {
  pending = state
}

// 只读查看：开发模式的 StrictMode 会重复调用渲染与 effect，读取与清除必须分开。
export function peekBoardReturn() {
  return pending
}

export function clearBoardReturn() {
  pending = null
}
