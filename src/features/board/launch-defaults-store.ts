// 启动台上次的选择：项目、模型、思考档位、审批模式与运行模式（Plan / 目标 / 团队）。
// 只保存选择本身；模型是否仍可用、目录是否仍存在由启动台在派发前向 Runtime 校验。
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

export type LaunchDefaults = {
  cwd: string
  model: string
  thinkingLevel: string
  executionMode: LaunchExecutionMode | ''
  runMode: LaunchRunMode
}

type LaunchDefaultsState = LaunchDefaults & {
  rememberLaunch: (choice: Partial<LaunchDefaults>) => void
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
    executionMode: LAUNCH_EXECUTION_MODES.includes(source.executionMode as LaunchExecutionMode)
      ? (source.executionMode as LaunchExecutionMode)
      : '',
    runMode: LAUNCH_RUN_MODES.includes(source.runMode as LaunchRunMode)
      ? (source.runMode as LaunchRunMode)
      : 'plan',
  }
}

export const useLaunchDefaultsStore = create<LaunchDefaultsState>()(
  persist(
    (set, get) => ({
      ...DEFAULT_LAUNCH_DEFAULTS,
      rememberLaunch: (choice) => {
        const { cwd, model, thinkingLevel, executionMode, runMode } = get()
        set(
          normalizeLaunchDefaults({ cwd, model, thinkingLevel, executionMode, runMode, ...choice }),
        )
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
