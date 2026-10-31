// @public 会话路由（应用壳）用的看板领域组件；只在会话路由里懒加载，不进入入口包。
// 桌面泳道条与手机代理条都已退役（规格 v2：并行的代理在应用侧栏 / 手机抽屉里）。
// 会话主页的“新任务”草稿（会话页外壳 + 空转录 + 启动台输入框），随会话路由懒加载。
export { NewTaskDraft, type NewTaskDraftProps } from './NewTaskDraft'
