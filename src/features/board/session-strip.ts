// @public 会话路由（应用壳）用的看板领域组件；只在会话路由里懒加载，不进入入口包。
// 桌面的泳道条已退役（规格 v2：并行的代理在应用侧栏里）；手机外壳会话页的代理条（规格 7.3）留到 A4。
export { PhoneAgentStrip, type PhoneAgentStripProps } from './PhoneAgentStrip'
export { effectiveStripCurrent, type PendingStripSwitch } from './strip-layout'
// 会话主页的“新任务”草稿（会话页外壳 + 空转录 + 启动台输入框），随会话路由懒加载。
export { NewTaskDraft, type NewTaskDraftProps } from './NewTaskDraft'
