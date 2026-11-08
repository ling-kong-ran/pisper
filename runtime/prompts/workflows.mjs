// 工作流节点交给 Agent 的指令：节点类型提示、输出要求、工作流输入与前序结果。
// 纯函数，只拼文字；模板渲染与序列化由调用方先做好再传进来。

const KIND_HINTS = {
  prompt: () => '',
  skill: (node) =>
    node.skillName ? `Use the skill "${node.skillName}".` : 'Use the best enabled skill.',
  file: () => 'Use the file tools.',
  mcp: (node) =>
    node.requestedToolNames.length
      ? `Prefer these MCP tools: ${node.requestedToolNames.join(', ')}.`
      : 'Prefer the enabled MCP tools.',
}

/**
 * @param {{
 *   workflowName: string,
 *   node: { kind: string, label: string, outputFormat?: string, skillName?: string, requestedToolNames: string[] },
 *   prompt: string,
 *   inputs: string,
 *   mediaContext?: string,
 *   previous: { label: string, text: string }[],
 * }} input
 */
export function workflowNodeInstruction({
  workflowName,
  node,
  prompt,
  inputs,
  mediaContext = '',
  previous,
}) {
  return [
    `You are running node "${node.label}" of workflow "${workflowName}".`,
    KIND_HINTS[node.kind]?.(node) || '',
    prompt,
    inputs ? `\nWorkflow inputs:\n${inputs}` : '',
    mediaContext,
    previous.length
      ? `\nEarlier node results:\n${previous.map((item) => `${item.label}: ${item.text}`).join('\n')}`
      : '',
    node.outputFormat === 'json'
      ? '\nOutput valid JSON only, without a Markdown code block.'
      : '\nEnd with a short summary of the result for the next nodes.',
  ]
    .filter(Boolean)
    .join('\n')
}

// 节点运行记录里的摘要：界面上展示，也会作为前序结果交给下游节点。
export const WORKFLOW_NODE_SUMMARY = Object.freeze({
  completed: 'Node completed.',
  branchNotSelected: 'Skipped: upstream branch not selected.',
  workflowCompleted: 'Workflow completed.',
  workflowStopped: 'Workflow stopped.',
  upstreamFailed: 'Skipped: an upstream node failed.',
  triggered: 'Workflow triggered.',
  conditionTrue: 'Condition met; took the true branch.',
  conditionFalse: 'Condition not met; took the false branch.',
  parallelJoined: 'Parallel branches joined.',
  parallelStarted: 'Parallel branches started.',
  approved: 'Approved.',
  notificationSent: 'Notification sent.',
  controlPassed: 'Control node passed.',
  skipped: (message) => `Skipped: ${message}`,
  interruptedByRestart: 'Interrupted because the app restarted.',
})

export function workflowApprovalQuestion(label) {
  return `Allow the workflow to continue with "${label}"?`
}
