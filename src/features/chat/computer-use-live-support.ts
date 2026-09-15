// Computer Use 实时镜像的纯逻辑：工具名识别与目标活动挑选。
// 从组件文件拆出，保持 React Fast Refresh 只包含组件导出。
import type { EntityRecord } from '@/types/chat'

// 识别 computer use 工具：pi-computer-use 注册的全部桌面自动化工具名。
const COMPUTER_USE_TOOL_NAMES = new Set([
  'find_roots',
  'observe_ui',
  'search_ui',
  'expand_ui',
  'inspect_ui',
  'act_ui',
  'read_text',
  'wait_for',
  'launch_browser',
  'navigate_browser',
  'evaluate_browser',
])

export function isComputerUseToolName(name: unknown): boolean {
  return COMPUTER_USE_TOOL_NAMES.has(String(name || ''))
}

// 会话中最后一个带目标窗口的 computer use 活动：镜像跟随 agent 当前操作的窗口。
export function latestComputerUseTarget(activities: EntityRecord[]): EntityRecord | null {
  for (let index = activities.length - 1; index >= 0; index -= 1) {
    const activity = activities[index]
    if (
      activity?.type === 'tool' &&
      isComputerUseToolName(activity.name) &&
      Number(activity.target?.windowId) > 0
    ) {
      return activity
    }
  }
  return null
}
