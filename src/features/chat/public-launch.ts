// @public 看板启动台与卡片复用的聊天能力：推荐任务、未发送草稿与会话改动统计。
// 只转出这几项，看板不触碰聊天页的会话状态与流结算；没有导入副作用。
export { default as QuickPromptIdeas } from './QuickPromptIdeas'
export { clearComposerDraft, readComposerDraft, updateComposerDraft } from './composer-drafts'
export { getSessionChangeSummary, type SessionChangeSummary } from './session-change-summary-api'
