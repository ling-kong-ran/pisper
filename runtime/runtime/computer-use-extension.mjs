import { fileURLToPath } from 'node:url'

const OFFICIAL_EXTENSION_ENTRY = '@injaneity/pi-computer-use/extensions/computer-use.ts'

// 对外只展示一个聚合工具；调用时再展开为官方 Extension 注册的真实工具名。
export const OFFICIAL_COMPUTER_USE_TOOL_NAMES = [
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
]

export function getOfficialComputerUseExtensionPath() {
  return fileURLToPath(import.meta.resolve(OFFICIAL_EXTENSION_ENTRY))
}
