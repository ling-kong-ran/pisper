// @public 轻量契约：应用壳无须加载看板数据层即可使用；不导入页面，也没有导入副作用。
// 新任务统一进入新任务草稿 '/board/new'（见 App.startNewTask），派发浮层与“视图认领新任务”的事件已退役。
// 全部代理页“/ 聚焦筛选”的目标：侧栏状态筛选的第一项“全部”（规格 2.3、2.8）。
// 应用壳的 focusSearch 在全部代理页聚焦它；侧栏收成窄轨时先临时展开。
export const BOARD_FILTER_ID = 'board-filter'
