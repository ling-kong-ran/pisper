// 工作室的写操作（计划 §4.4）：模板创建、启用、删除、放置 / 移出、实例属性与范围。
// 全部经 custom-ui-api 的请求函数；成功后作废 ['custom-ui'] 下的查询（目录、实例、describe、errors），
// 组件变化事件（component_changed）也会再作废一次，两条路径结果一致。
// 失败统一换成错误码文案提示；属性保存的字段级错误交给调用方定位到字段，不在这里吞掉。
import { useMemo } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import type { ComponentSurface, OverviewCardSize } from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import {
  componentErrorCode,
  createComponentFromTemplate,
  createComponentInstance,
  deleteComponent,
  deleteComponentInstance,
  setComponentEnabled,
  updateComponentInstance,
  type ComponentInstance,
  type ComponentInstanceScope,
  type ComponentRow,
} from '@/features/custom-ui/custom-ui-api'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'
import { componentErrorText } from './error-text'

export type StudioTemplate = 'blank' | 'usage-today' | 'notes'

export function useStudioActions(notify: (message: string) => void) {
  const client = useQueryClient()
  const { t } = useI18n()
  return useMemo(() => {
    const refresh = () => client.invalidateQueries({ queryKey: customUiQueryKeys.all })
    // 操作失败：有组件错误码就显示对应文案，否则显示这次操作的通用失败提示。
    const fail = (error: unknown, fallback: string) => {
      const code = componentErrorCode(error)
      notify(code ? `${fallback} ${componentErrorText(code, t)}` : fallback)
    }
    const run = async <T>(task: () => Promise<T>, failure: string): Promise<T | null> => {
      try {
        const result = await task()
        await refresh()
        return result
      } catch (error) {
        fail(error, failure)
        return null
      }
    }
    return {
      refresh,
      createFromTemplate: (template: StudioTemplate, name?: string) =>
        run(
          () => createComponentFromTemplate({ template, ...(name ? { name } : {}) }),
          t('custom-ui:studio.createFailed'),
        ),
      setEnabled: (row: ComponentRow, enabled: boolean) =>
        run(() => setComponentEnabled(row.id, enabled), t('custom-ui:studio.toggleFailed')),
      remove: async (row: ComponentRow) => {
        const result = await run(() => deleteComponent(row.id), t('custom-ui:studio.deleteFailed'))
        if (result) notify(t('custom-ui:studio.deleted', { name: row.name }))
        return result
      },
      // 放置：单实例放置（会话面板、功能页、浮窗）已有但被关掉（浮窗关闭 = enabled:false）时重新打开，
      // 不再建第二个（Runtime 会 409）；总览卡片每次都新建一张。
      place: (
        row: ComponentRow,
        instances: readonly ComponentInstance[],
        input: {
          surface: ComponentSurface
          size?: OverviewCardSize
          scope?: ComponentInstanceScope
          // 换个位置（总览卡片 ⋯）：新建时带上原卡片的标题与属性；重新打开已有实例时保留它自己的。
          title?: string
          props?: Record<string, unknown>
        },
      ) => {
        const existing =
          input.surface === 'overview-card'
            ? undefined
            : instances.find(
                (instance) => instance.componentId === row.id && instance.surface === input.surface,
              )
        return run(
          () =>
            existing
              ? updateComponentInstance(existing.instanceId, {
                  enabled: true,
                  ...(input.scope ? { scope: input.scope } : {}),
                })
              : createComponentInstance({ componentId: row.id, ...input }),
          t('custom-ui:studio.placeFailed'),
        )
      },
      unplace: (instance: ComponentInstance) =>
        run(
          () => deleteComponentInstance(instance.instanceId),
          t('custom-ui:studio.unplaceFailed'),
        ),
      patchInstance: (
        instance: ComponentInstance,
        patch: { size?: OverviewCardSize; scope?: ComponentInstanceScope | null },
      ) =>
        run(
          () => updateComponentInstance(instance.instanceId, patch),
          t('custom-ui:studio.saveFailed'),
        ),
      // 属性保存：失败原样抛出，由属性表单按 errors 定位到字段。
      saveProps: async (
        instance: ComponentInstance,
        patch: { title: string; props: Record<string, unknown> },
      ) => {
        const result = await updateComponentInstance(instance.instanceId, patch)
        await refresh()
        return result
      },
    }
  }, [client, notify, t])
}
