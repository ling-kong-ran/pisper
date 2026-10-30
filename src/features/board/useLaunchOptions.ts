// 启动台的可选项：已配置的对话模型（含各自的思考档位）、默认模型、最近用过的项目（含还没有会话的
// 登记项目），以及 Runtime 是否支持目标 / 团队模式。全部读现有缓存（config 与会话目录）与本机偏好，
// 不新开轮询。
import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useRegisteredProjectsStore } from '@/features/chat/registered-projects-store'
import { useWorkspaceOrderStore } from '@/features/chat/workspace-order-store'
import { startupQueryOptions } from '@/lib/startup-queries'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import type { EntityRecord } from '@/types/chat'
import { defaultLaunchCwd, recentLaunchProjects, type LaunchProject } from './board-projects'
import { useSessionListQuery } from './useSessionListQuery'

export type LaunchModelOption = {
  key: string
  label: string
  providerName: string
  thinkingLevels: string[]
}

export type LaunchOptions = {
  models: LaunchModelOption[]
  defaultModel: string
  // Runtime 的默认思考档位（/api/config 的 thinkingLevel）；没选档位时芯片按它显示，与聊天页同口径。
  defaultThinkingLevel: string
  projects: LaunchProject[]
  // 没有记住派发目标时的默认目录（最近有会话的项目优先，见 defaultLaunchCwd）。
  defaultCwd: string
  goalsAvailable: boolean
  teamAvailable: boolean
  loading: boolean
  // config 读取失败：芯片仍可用（沿用 Runtime 默认），只是列不出模型。
  error: unknown
}

type ConfigData = EntityRecord & {
  provider?: string
  model?: string
  thinkingLevel?: string
  providers?: unknown
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {}
}

function text(value: unknown) {
  return typeof value === 'string' ? value : ''
}

// 与聊天页的模型列表同一口径：只取已配置且启用的 Provider 下的对话模型。
export function launchModelOptions(config: unknown): LaunchModelOption[] {
  const providers = Array.isArray(record(config).providers)
    ? (record(config).providers as unknown[])
    : []
  return providers.flatMap((value) => {
    const provider = record(value)
    const id = text(provider.id)
    if (!id || provider.configured !== true || provider.enabled === false) return []
    const models = Array.isArray(provider.models) ? provider.models : []
    return models.flatMap((item) => {
      const model = record(item)
      const modelId = text(model.id)
      if (!modelId || model.kind !== 'chat') return []
      return [
        {
          key: `${id}/${modelId}`,
          label: text(model.name) || modelId,
          providerName: text(provider.name) || id,
          thinkingLevels: Array.isArray(model.thinkingLevels)
            ? model.thinkingLevels.filter((level): level is string => typeof level === 'string')
            : [],
        },
      ]
    })
  })
}

export function useLaunchOptions(): LaunchOptions {
  const config = useQuery(startupQueryOptions<ConfigData>('config'))
  const sessions = useSessionListQuery()
  const names = useWorkspaceOrderStore((state) => state.names)
  const registered = useRegisteredProjectsStore((state) => state.projects)
  const features = useRuntimeCapabilitiesStore((state) => state.capabilities.features)
  const models = useMemo(() => launchModelOptions(config.data), [config.data])
  const defaultModel =
    config.data?.provider && config.data?.model
      ? `${config.data.provider}/${config.data.model}`
      : ''
  const list = sessions.data?.sessions
  const projects = useMemo(
    () => recentLaunchProjects(list ?? [], registered, names),
    [list, names, registered],
  )
  const defaultCwd = useMemo(() => defaultLaunchCwd(list ?? [], projects), [list, projects])
  return {
    models,
    defaultModel,
    defaultThinkingLevel: text(config.data?.thinkingLevel) || 'medium',
    projects,
    defaultCwd,
    goalsAvailable: features.goals !== false,
    teamAvailable: features.goals !== false && features.multiAgent !== false,
    loading: config.isPending,
    error: config.error ?? null,
  }
}
