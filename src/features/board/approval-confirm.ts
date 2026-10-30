// @public 高风险批准的确认层（按住 0.6 秒或层内 ↵）：会话页审批面板与看板共用同一个组件，
// 保证 critical 审批在任何入口都不能一步批准。文案在 board 命名空间，调用方负责先加载它。
export { ApprovalConfirm, type ApprovalConfirmTarget } from './ApprovalConfirm'
export type { BoardApproval as ConfirmableApproval } from './board-api'
