// @public 原生组件放置的轻量入口（计划 §4.1、§4.6）：capability 门控 + react-query + 统一 HTTP 客户端。
// 放置宿主（全部代理右栏、功能 › 我的组件、会话检查器、浮窗）只静态导入这个文件；宿主本体（htm、native/*）
// 经 LazyComponentHost 的懒加载边界进入，实例数为 0 时永远不会加载。这里不导入 shared/custom-ui-manifest.mjs、
// htm 或 native/*（frontend-boundaries 守卫）。
import { lazy, useEffect, useMemo } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import type { ComponentSurface } from '@shared/custom-ui-manifest.mjs'
import { ensureCustomUiMessages } from '@/app/i18n'
import { useI18n } from '@/app/use-i18n'
import { subscribeComponentChanges } from '@/features/board/component-changes'
import { useRuntimeCapabilitiesStore } from '@/stores/runtime-capabilities-store'
import { createComponentChangeSync } from './change-sync'
import {
  listComponentCatalog,
  listComponentInstances,
  type ComponentInstance,
  type ComponentRow,
} from './custom-ui-api'
import { customUiQueryKeys } from './query-keys'

export type { ComponentCatalog, ComponentInstance, ComponentRow } from './custom-ui-api'

const changeSync = createComponentChangeSync(subscribeComponentChanges)

function useComponentChangeSync(enabled: boolean) {
  const client = useQueryClient()
  useEffect(() => (enabled ? changeSync.retain(client) : undefined), [client, enabled])
}

/** Runtime 声明了 components.native（新 Runtime）才请求组件接口；旧 Runtime 上四个宿主都不请求实例。 */
export function useComponentsAvailable(): boolean {
  return useRuntimeCapabilitiesStore((state) => state.capabilities.features['components.native'])
}

/** 目录查询的共享选项：所有放置与组件页读同一份缓存，最后一个使用者离开时才取消请求。 */
export function componentCatalogQueryOptions(enabled = true) {
  return {
    queryKey: customUiQueryKeys.catalog(),
    queryFn: ({ signal }: { signal?: AbortSignal }) => listComponentCatalog(signal),
    enabled,
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  } as const
}

export function useComponentCatalog() {
  const available = useComponentsAvailable()
  useComponentChangeSync(available)
  return useQuery(componentCatalogQueryOptions(available))
}

function useInstancesQuery(available: boolean) {
  useComponentChangeSync(available)
  return useQuery({
    queryKey: customUiQueryKeys.instances(),
    queryFn: ({ signal }) => listComponentInstances(signal),
    enabled: available,
    staleTime: 30_000,
    retry: false,
    refetchOnWindowFocus: false,
  })
}

/** 全部实例（含已停用的）：组件工作室按组件列出放置与属性；与放置宿主共用同一份缓存。 */
export function useComponentInstanceList() {
  return useInstancesQuery(useComponentsAvailable())
}

export type PlacedComponent = { instance: ComponentInstance; component: ComponentRow }

/**
 * 某个放置位置上可以渲染的实例：实例已启用，组件是原生、ready、已启用。按 order 排序。
 * 没有实例时不请求目录（宿主据此不渲染、不加载懒加载 chunk）。
 */
export function useComponentInstances(surface: ComponentSurface) {
  const available = useComponentsAvailable()
  const instances = useInstancesQuery(available)
  const candidates = useMemo(
    () =>
      (instances.data?.instances ?? []).filter(
        (instance) => instance.surface === surface && instance.enabled,
      ),
    [instances.data, surface],
  )
  const catalog = useQuery(componentCatalogQueryOptions(available && candidates.length > 0))
  const placed = useMemo(() => {
    if (!catalog.data) return []
    const rows = new Map(catalog.data.components.map((row) => [row.id, row]))
    const result: PlacedComponent[] = []
    for (const instance of candidates) {
      const component = rows.get(instance.componentId)
      if (component?.kind === 'native' && component.status === 'ready' && component.enabled)
        result.push({ instance, component })
    }
    return result
  }, [candidates, catalog.data])
  return {
    available,
    placed,
    revision: instances.data?.revision ?? null,
    isLoading: available && (instances.isPending || (candidates.length > 0 && catalog.isPending)),
    error: instances.error ?? catalog.error ?? null,
    refetch: () => {
      void instances.refetch()
      if (candidates.length) void catalog.refetch()
    },
  }
}

/** 已启用的实例数（任何放置位置）：应用壳据此决定要不要懒加载安全模式询问。没有实例时什么都不加载。 */
export function useEnabledComponentInstanceCount(): number {
  const available = useComponentsAvailable()
  const instances = useInstancesQuery(available)
  return useMemo(
    () => (instances.data?.instances ?? []).filter((instance) => instance.enabled).length,
    [instances.data],
  )
}

/** 功能 › 我的组件：feature-page 实例 join 目录，只要可渲染的，名称按当前语言选。 */
export function useFeaturePageComponents() {
  const { language } = useI18n()
  const result = useComponentInstances('feature-page')
  const items = useMemo(
    () =>
      result.placed.map(({ instance, component }) => ({
        instanceId: instance.instanceId,
        componentId: component.id,
        name: component.manifest?.i18n[language]?.name || component.name,
        description: component.manifest?.i18n[language]?.description || component.description,
        icon: component.manifest?.icon || '',
      })),
    [language, result.placed],
  )
  return { ...result, items }
}

// 宿主本体与 custom-ui 文案一起按需加载；放置宿主只在有实例时渲染它。
const loadHosts = () =>
  Promise.all([import('./hosts'), ensureCustomUiMessages()]).then(([module]) => module)

export const LazyComponentHost = lazy(() =>
  loadHosts().then((module) => ({ default: module.ComponentHost })),
)
// 浮窗放置：floating 实例数 > 0 且不在手机外壳时由应用壳渲染。
export const LazyFloatingComponents = lazy(() =>
  loadHosts().then((module) => ({ default: module.FloatingComponents })),
)
// 安全模式询问：上次加载组件时可能卡死（哨兵还在）才显示；有已启用实例时由应用壳渲染。
export const LazySafeModePrompt = lazy(() =>
  loadHosts().then((module) => ({ default: module.SafeModePrompt })),
)
