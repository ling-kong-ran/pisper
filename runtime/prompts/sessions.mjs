// 会话生命周期里交给模型的短句：审批因会话状态被结束时的理由、ask_user 被取消的理由、
// 只带附件时的默认提示。集中在这里，模块里只留调用；纯常量，无 I/O。

// 只发附件、不写文字时代替用户消息的提示（也显示为这条用户消息的文本）。
export const ATTACHMENTS_ONLY_PROMPT = 'Please analyze these attachments.'

// 待审批工具调用被会话状态结束时，工具结果里给模型的理由。
export const APPROVAL_SETTLED = Object.freeze({
  runtimeReleased: 'Session runtime was released; resend the message.',
  stopped: 'Session stopped; the tool did not run.',
  deleted: 'Session deleted; the tool did not run.',
  fullAccess: 'Switched to full access.',
  executionModeChanged: 'Execution mode changed; retry the tool call under the new permissions.',
  /** @param {string} mode */
  permissionMode: (mode) => `Permission mode switched to ${mode === 'ignore' ? 'ignore' : 'auto'}.`,
})

// ask_user 的问题被会话状态取消时给模型的理由。
export const QUESTION_CANCELLED = Object.freeze({
  runtimeReleased: 'Session runtime was released.',
  stopped: 'Run stopped.',
  deleted: 'Session deleted.',
})

// 压缩保留段里，同一文件较早的读取结果被后面的读取取代时，替换原正文的占位。
/** @param {string} path */
export const SUPERSEDED_READ = (path) =>
  `[Earlier read of ${path} omitted: a later read below shows the current content.]`
