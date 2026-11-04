import { MULTI_AGENT_TOOL_NAMES } from '../services/multi-agent-service.mjs'
import { GOAL_TOOL_NAMES } from '../tools/app/goal.mjs'
import { ASK_USER_TOOL_NAME } from '../tools/app/ask-user.mjs'
import { SCHEDULE_TOOL_NAMES } from '../tools/app/schedules.mjs'
import { WORKFLOW_TOOL_NAMES } from '../tools/app/workflows.mjs'
import { PLAN_ALL_TOOL_NAMES, PLAN_COMPATIBILITY_TOOL_NAMES } from '../tools/app/plan.mjs'
import { TOOL_DISCOVERY_NAME } from '../tools/app/tool-discovery.mjs'
import { TOOL_GATEWAY_NAME } from '../tools/app/tool-gateway.mjs'
import { BOT_CONVERSATION_TOOL_NAMES } from '../tools/app/bot-conversation-tool-names.mjs'
import { selectedToolNames } from '../tools/tool-activation.mjs'
import { applyPisperSystemPrompt } from '../prompts/pisper-system-prompt.mjs'

// 限定了工具范围的运行（Bot 的 tools）也始终保留的基础设施：发现与网关本身、计划、目标与提问。
// 网关只是入口，真正要调用的工具仍按范围检查。
const SCOPE_INFRA_TOOL_NAMES = [
  TOOL_DISCOVERY_NAME,
  TOOL_GATEWAY_NAME,
  ...PLAN_ALL_TOOL_NAMES,
  ...GOAL_TOOL_NAMES,
  ASK_USER_TOOL_NAME,
]

// Bot 的对话（v2.1）用自己的经理工具（bot_*）代替这些：通用的多 Agent、提问、定时、工作流与目标工具会让它
// 阻塞（ask_user 等回答、目标循环）或绕过 Bot 的设定（派出的子 agent 不带 Bot 的范围与记忆）；浏览器自动化太慢，交给子 agent。
export const BOT_CONVERSATION_BLOCKED_TOOLS = Object.freeze([
  ...MULTI_AGENT_TOOL_NAMES,
  ASK_USER_TOOL_NAME,
  ...GOAL_TOOL_NAMES,
  ...SCHEDULE_TOOL_NAMES,
  ...WORKFLOW_TOOL_NAMES,
  'browser_automation',
])

// 本轮不可用的工具：隔离上下文的屏蔽项；没人在场的运行里的 ask_user、定时任务与工作流工具
// （定时任务、工作流本身的运行不能再建定时任务或启动工作流，避免自我复制）；
// 本轮限定的工具范围（allowedToolNames，null 表示不限定）之外的工具；远程会话里大脑本机才有的工具。
// 激活与发现用它隐藏工具，权限服务用它在执行前拦下——经网关按名字直接调用也绕不过去。
export function runToolBlocker(value) {
  const blocked = new Set(value?.blockedToolNames || [])
  if (value?.unattendedRun) {
    blocked.add(ASK_USER_TOOL_NAME)
    for (const name of [...SCHEDULE_TOOL_NAMES, ...WORKFLOW_TOOL_NAMES]) blocked.add(name)
  }
  const allowed = Array.isArray(value?.allowedToolNames)
    ? new Set([...value.allowedToolNames, ...SCOPE_INFRA_TOOL_NAMES])
    : null
  // 远程会话（B6）只有能交给那台电脑执行的工具。
  const remote = Array.isArray(value?.executorToolNames)
    ? new Set([...value.executorToolNames, ...SCOPE_INFRA_TOOL_NAMES])
    : null
  return (name) =>
    blocked.has(name) ||
    (allowed !== null && !allowed.has(name)) ||
    (remote !== null && !remote.has(name))
}

// 工具激活策略：根据目标状态/用户请求动态启用工具，并同步系统提示；执行模式不影响工具集合。
// 工具在会话中可能被“提升”（promoted）——即模型可直接调用，而非常驻可见。
export class ToolActivation {
  constructor({ getSessionMeta, saveSessionMeta, getGoal, promoteTools }) {
    this.getSessionMeta = getSessionMeta
    this.saveSessionMeta = saveSessionMeta
    this.getGoal = getGoal
    this.promoteTools = promoteTools
  }

  // 可选工具集合：基础工具 + 计划兼容工具 + 多 Agent 工具，扣除隔离上下文屏蔽项。
  optionalToolNames(value) {
    const isBlocked = runToolBlocker(value)
    return [
      ...(value?.baseToolNames || []),
      ...(value?.session?.getAllTools?.() || []).map((tool) => tool.name),
      ...PLAN_COMPATIBILITY_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
    ].filter((name) => !isBlocked(name))
  }

  // 同步目标工具开关：目标激活时启用目标相关工具，否则关闭。
  syncGoalTools(value, goal) {
    if (!value?.session) return
    const isBlocked = runToolBlocker(value)
    const availableToolNames = [
      ...(value.baseToolNames || []),
      TOOL_DISCOVERY_NAME,
      TOOL_GATEWAY_NAME,
      ...PLAN_ALL_TOOL_NAMES,
      ...MULTI_AGENT_TOOL_NAMES,
      ...GOAL_TOOL_NAMES,
      ASK_USER_TOOL_NAME,
      // Bot 的对话：自己动手的工具按 Bot 的范围，外加它当经理用的 bot_* 工具。
      ...(value.conversationOnly ? BOT_CONVERSATION_TOOL_NAMES : []),
    ].filter((name) => !isBlocked(name))
    const names = selectedToolNames({
      availableToolNames,
      promotedToolNames: [],
      requestedToolNames: [],
      goalToolNames: GOAL_TOOL_NAMES,
      goalActive: goal?.status === 'active',
    })
    value.session.setActiveToolsByName(names)
  }

  // 提升指定工具：只路由本会话可用的工具，同步目标工具与系统提示。
  async promoteSessionTools(value, toolNames = []) {
    if (!value?.session) return { routedToolNames: [] }
    const availableToolNames = this.optionalToolNames(value)
    const routedToolNames = [
      ...new Set(toolNames.filter((name) => availableToolNames.includes(name))),
    ]
    this.syncGoalTools(value, this.getGoal(value.session.sessionId))
    applyPisperSystemPrompt(value.session, value.session.model)
    return { routedToolNames }
  }

  // 按消息请求选择工具：累积用户显式请求的工具名，并刷新激活工具集。
  async selectToolsForMessage(
    value,
    _message,
    { requestedToolNames = [], preserveRequested = false } = {},
  ) {
    if (!value?.session) return []
    const requested = [
      ...new Set(
        (Array.isArray(requestedToolNames) ? requestedToolNames : [])
          .map((name) => String(name || '').trim())
          .filter(Boolean),
      ),
    ]
    const routed = await this.promoteTools(value, requested)
    value.requestedToolNames = preserveRequested
      ? [...new Set([...(value.requestedToolNames || []), ...(routed.routedToolNames || [])])]
      : routed.routedToolNames || []
    return value.session.getActiveToolNames()
  }
}
