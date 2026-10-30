// @public 轻量契约：应用壳把 ⌘J / ⌥A / ⌥↑⌥↓ 交给侧栏会话分组处理，不必为此把看板数据层放进入口包。
// 分组（随看板 store 懒加载）挂载后监听这个事件；还没加载时事件无人接收，按键不做任何事。
// 不导入任何模块，也没有导入副作用。

export const SIDEBAR_COMMAND_EVENT = 'pisper:sidebar-command'

export type SidebarCommand =
  // ⌘J：聚焦侧栏会话列表（当前会话行，没有时第一行）；收起时临时展开。
  | { kind: 'focus-list' }
  // ⌥A：聚焦等待最久的需要你行（只移动焦点，不批准）；收起时临时展开。
  | { kind: 'focus-attention' }
  // ⌥↑ / ⌥↓：按侧栏可见顺序切换到上一个 / 下一个会话。
  | { kind: 'adjacent'; direction: 1 | -1 }

export function requestSidebarCommand(command: SidebarCommand) {
  window.dispatchEvent(new CustomEvent<SidebarCommand>(SIDEBAR_COMMAND_EVENT, { detail: command }))
}
