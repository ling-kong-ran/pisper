// @public 会话页（应用壳的会话路由）用的泳道条；只在会话路由里懒加载，不进入入口包。
export { SessionLaneStrip, type SessionLaneStripProps } from './SessionLaneStrip'
// 手机外壳会话页的代理条（规格 7.3），同样只随会话路由加载。
export { PhoneAgentStrip, type PhoneAgentStripProps } from './PhoneAgentStrip'
export { effectiveStripCurrent, type PendingStripSwitch } from './strip-layout'
// 会话主页的“新任务”草稿（会话页外壳 + 空转录 + 启动台输入框），随会话路由懒加载。
export { NewTaskDraft, type NewTaskDraftProps } from './NewTaskDraft'
