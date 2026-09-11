import { fileURLToPath } from 'node:url'
import { OCR_LANGUAGES, OCR_MODEL_VERSION } from '../../shared/ocr-model-catalog.mjs'

const OFFICIAL_EXTENSION_ENTRY = '@injaneity/pi-computer-use/extensions/computer-use.ts'

export const COMPUTER_USE_OCR_LANGUAGES = Object.freeze(['eng', 'chi_sim', 'chi_sim+eng'])
export const COMPUTER_USE_OCR_MODEL_VERSION = OCR_MODEL_VERSION
export const COMPUTER_USE_OCR_MODEL_FILES = Object.freeze([...OCR_LANGUAGES])

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

export function getComputerUseOcrExtensionPath() {
  return fileURLToPath(new URL('../extensions/computer-use-ocr.mjs', import.meta.url))
}
