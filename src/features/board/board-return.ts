// 从全部代理（或手机看板）打开会话前记下的位置：列表滚动位置与来源行。回到全部代理（侧栏入口、⌘1、
// 手机标签、浏览器后退）时页面重新挂载，据此恢复滚动并把焦点还给来源行。只保存在内存里，刷新页面后不恢复。
// 泳道条时期“卡片缩进芯片 / 从芯片展开”的 FLIP 交接已随泳道条（A1）与泳道（A3）退役。
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
