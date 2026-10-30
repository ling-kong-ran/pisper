// 启动台上次的选择：项目、模型、思考档位、审批模式与运行模式（Plan / 目标 / 团队）。
// 只保存选择本身；模型是否仍可用、目录是否仍存在由启动台在派发前向 Runtime 校验。
// “完全访问”会跳过所有审批，且偏好经 Runtime 同步到其他客户端：它从不写入这里，
// 只对启动台当前草稿生效；每次派发后回到上一次记住的安全模式（或 Runtime 默认的审批模式）。
import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import { boardPreferenceStorage, persistedRecord } from './board-preference-storage'

export const LAUNCH_DEFAULTS_PREFERENCE_KEY = 'pisper-launch-defaults'
// 审批模式沿用会话的 executionMode（Runtime 据此推导 permissionMode），空串表示用 Runtime 默认值。
export const LAUNCH_EXECUTION_MODES = [
  'approval-required',
  'workspace-write',
  'full-access',
] as const
// “Plan 模式”即聊天输入栏的 runMode = 'plan'，目标与团队模式同属这一选择。
export const LAUNCH_RUN_MODES = ['plan', 'goal', 'team'] as const

export type LaunchExecutionMode = (typeof LAUNCH_EXECUTION_MODES)[number]
export type LaunchRunMode = (typeof LAUNCH_RUN_MODES)[number]
// 可以被记住的审批模式：不含完全访问。
export type LaunchSafeExecutionMode = Exclude<LaunchExecutionMode, 'full-access'>

export const LAUNCH_SAFE_EXECUTION_MODES: readonly LaunchSafeExecutionMode[] =
  LAUNCH_EXECUTION_MODES.filter((mode): mode is LaunchSafeExecutionMode => mode !== 'full-access')

export type LaunchDefaults = {
  cwd: string
  model: string
  thinkingLevel: string
  executionMode: LaunchSafeExecutionMode | ''
  runMode: LaunchRunMode
}

// 只认安全模式；完全访问（包括旧版本已经写进偏好的值）与未知值一律回落到 Runtime 默认。
export function safeLaunchExecutionMode(value: unknown): LaunchSafeExecutionMode | '' {
  return LAUNCH_SAFE_EXECUTION_MODES.includes(value as LaunchSafeExecutionMode)
    ? (value as LaunchSafeExecutionMode)
    : ''
}

// 记住选择时可以把当次的审批模式原样传进来：完全访问会被忽略，保留上一次的安全模式。
export type LaunchChoice = Partial<Omit<LaunchDefaults, 'executionMode'>> & {
  executionMode?: LaunchExecutionMode | ''
}

type LaunchDefaultsState = LaunchDefaults & {
  rememberLaunch: (choice: LaunchChoice) => void
  resetLaunchDefaults: () => void
}

export const DEFAULT_LAUNCH_DEFAULTS: LaunchDefaults = {
  cwd: '',
  model: '',
  thinkingLevel: '',
  executionMode: '',
  runMode: 'plan',
}

function boundedText(value: unknown, maxLength: number, pattern?: RegExp) {
  if (typeof value !== 'string') return ''
  const text = value.trim()
  return text.length <= maxLength && (!pattern || pattern.test(text)) ? text : ''
}

export function normalizeLaunchDefaults(value: unknown): LaunchDefaults {
  const source = persistedRecord(value)
  return {
    cwd: boundedText(source.cwd, 4096),
    // provider/modelId；不在这里判断是否已配置，避免 Runtime 暂不可达时清掉用户的选择。
    model: boundedText(source.model, 256),
    // 未来档位名称未知，只接受短的小写标识。
    thinkingLevel: boundedText(source.thinkingLevel, 32, /^[a-z][a-z0-9_-]*$/),
    executionMode: safeLaunchExecutionMode(source.executionMode),
    runMode: LAUNCH_RUN_MODES.includes(source.runMode as LaunchRunMode)
      ? (source.runMode as LaunchRunMode)
      : 'plan',
  }
}

// 合并一次选择：完全访问不覆盖已记住的安全模式，其余字段照常归一化。
export function mergeLaunchChoice(current: LaunchDefaults, choice: LaunchChoice): LaunchDefaults {
  const next = normalizeLaunchDefaults({ ...current, ...choice })
  return choice.executionMode === 'full-access'
    ? { ...next, executionMode: safeLaunchExecutionMode(current.executionMode) }
    : next
}

export const useLaunchDefaultsStore = create<LaunchDefaultsState>()(
  persist(
    (set, get) => ({
      ...DEFAULT_LAUNCH_DEFAULTS,
      rememberLaunch: (choice) => {
        const { cwd, model, thinkingLevel, executionMode, runMode } = get()
        set(mergeLaunchChoice({ cwd, model, thinkingLevel, executionMode, runMode }, choice))
      },
      resetLaunchDefaults: () => set({ ...DEFAULT_LAUNCH_DEFAULTS }),
    }),
    {
      name: LAUNCH_DEFAULTS_PREFERENCE_KEY,
      storage: boardPreferenceStorage<LaunchDefaults>(),
      version: 1,
      partialize: ({ cwd, model, thinkingLevel, executionMode, runMode }) => ({
        cwd,
        model,
        thinkingLevel,
        executionMode,
        runMode,
      }),
      merge: (persisted, current) => ({ ...current, ...normalizeLaunchDefaults(persisted) }),
    },
  ),
)
