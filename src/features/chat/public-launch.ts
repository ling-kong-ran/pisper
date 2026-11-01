// @public 看板启动台与卡片复用的聊天能力：推荐任务、未发送草稿、会话改动统计与斜杠命令菜单。
// 只转出这几项，看板不触碰聊天页的会话状态与流结算；没有导入副作用。
import { chatApi } from './chat-api'

export { default as QuickPromptIdeas } from './QuickPromptIdeas'
export { ComposerCommandMenu } from './ComposerCommandMenu'
export { runtimeSettingCommands, type BuiltinCommand } from './builtin-commands'
export const getWorkspaceCommands = (cwd: string) => chatApi.getWorkspaceCommands(cwd)
export { clearComposerDraft, readComposerDraft, updateComposerDraft } from './composer-drafts'
export { getSessionChangeSummary, type SessionChangeSummary } from './session-change-summary-api'
