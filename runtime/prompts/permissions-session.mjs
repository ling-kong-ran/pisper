// 会话审批给模型和审批卡片看的文字：为什么要确认、为什么没执行、审批如何结束。
// 纯函数与常量，只拼文字；风险判定留在 runtime/services/session-permission-service.mjs。

// 命令守卫拦截的固定前缀在 shared/session-board-status.mjs：危险等级判定和前端摘要都靠它识别守卫原因。
import { COMMAND_GUARD_REASON_PREFIX } from '../../shared/session-board-status.mjs'

const CONFIRM = 'Needs confirmation.'

export const PERMISSION_REASON = Object.freeze({
  teamNoPath: (tool) => `${tool} has no target path, so Team file ownership cannot be checked.`,
  teamOutside: (tool, scopes, received) =>
    `${tool} can only change files owned by this Team task: ${scopes.join(', ')}. Got ${received}.`,
  globalSkill: `skill_create will create a global skill available to every project. ${CONFIRM}`,
  pluginCreate: `plugin_create will create and install a global plugin. ${CONFIRM}`,
  outsideWorkspace: (tool) => `${tool} will access files outside the working folder. ${CONFIRM}`,
  schedule:
    'schedule_manage will schedule an unattended task that runs with full access. Check the task and time before confirming.',
  workflowStart:
    'workflow_run will start a workflow whose nodes run unattended with full access. Check the workflow and inputs before confirming.',
  guard: (reason) => `${COMMAND_GUARD_REASON_PREFIX}: ${reason}.`,
  guardConfirm: (reason) => `${COMMAND_GUARD_REASON_PREFIX}: ${reason}. ${CONFIRM}`,
  computerUse: `Computer Use will see your screen and control the mouse and keyboard. ${CONFIRM}`,
  browserInteraction: `Browser interaction may submit forms or change remote state. ${CONFIRM}`,
  thirdParty: (tool) =>
    `${tool} comes from a third-party plugin and runs local code as the current user. ${CONFIRM}`,
  workspaceWrite: (tool) => `${tool} will change files in the working folder. ${CONFIRM}`,
  componentCreate: (id) =>
    `component_create will write component ${id} (data folder/custom-ui/${id}). ${CONFIRM}`,
  invalidComponentId: '(invalid id)',
  shell:
    'Shell commands run as the current OS user; once approved they can reach files outside the working folder and the network.',
  riskTool: (tool, risk) => `${tool} is a ${risk}-risk tool. ${CONFIRM}`,
})

// 没执行的原因：模型据此决定换个办法或告诉用户。
export const PERMISSION_RESULT = Object.freeze({
  toolOutOfScope: (tool) => `Not run: ${tool} is not available in this run.`,
  denyRule: (pattern) => `Not run: matches the deny rule "${pattern}".`,
  fileChangedDuringReview:
    'Not run: the target file changed during review. Request the change again to see the latest diff.',
  userDeniedTool: (tool) => `Not run: the user denied ${tool}.`,
  stopped: 'Not run: the operation was stopped.',
  timedOut: 'Not run: approval timed out.',
  approved: 'Approved by the user.',
  denied: 'Not run: the user denied this tool.',
  modeAllowed: 'Allowed by the permission mode.',
  sessionStopped: 'Session stopped.',
  appClosing: 'Not run: the app is closing.',
})

// 审批卡片与落盘的参数摘要里替换掉的内容。
export const ARGS_PLACEHOLDER = Object.freeze({
  omitted: '[omitted]',
  redacted: '[redacted]',
  omittedChars: (count) => `[omitted, ${count} characters]`,
})
