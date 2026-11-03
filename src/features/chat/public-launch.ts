// @public 看板启动台与卡片复用的聊天能力：未发送草稿、会话改动统计、斜杠命令菜单，
// 以及与会话页同一个的模型芯片（模型 · 思考强度）。
// 只转出这几项，看板不触碰聊天页的会话状态与流结算；没有导入副作用。
import { chatApi } from './chat-api'

export { ComposerCommandMenu } from './ComposerCommandMenu'
export { COMPOSER_CHIP_CLASSES } from './focus-session-composer-bits'
// 新任务草稿的契约：看板实现（项目芯片、派发偏好、排在…之后），会话页渲染。
export type { NewTaskDraftChrome, NewTaskProjectVariant } from './session-workspace-context'
export { ModelThinkingControl } from './FocusRuntimeControls'
export { runtimeSettingCommands, type BuiltinCommand } from './builtin-commands'
export const getWorkspaceCommands = (cwd: string) => chatApi.getWorkspaceCommands(cwd)
export { clearComposerDraft, readComposerDraft, updateComposerDraft } from './composer-drafts'
export { getSessionChangeSummary, type SessionChangeSummary } from './session-change-summary-api'
