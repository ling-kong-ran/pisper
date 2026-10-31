// 工作室的信任与生命周期（A8）：版本、导出、回收站、导入暂存与信任、能力台账的查询与写操作。
// 读取各自走 ['custom-ui', …] 下的查询键（组件变化事件会一并作废）；写操作成功后作废整个 ['custom-ui'] 前缀，
// 失败统一换成错误码文案提示。功能开关来自目录的 features（旧 Runtime 没有对应键时整段不显示）。
import { useMemo } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import type { CapabilityType } from '@shared/custom-ui-manifest.mjs'
import { useI18n } from '@/app/use-i18n'
import { componentErrorCode, type ComponentRow } from '@/features/custom-ui/custom-ui-api'
import {
  discardStagedComponent,
  exportComponentZip,
  getComponentUsage,
  getStagedComponent,
  listComponentTrash,
  listComponentVersions,
  purgeTrashEntry,
  restoreComponentVersion,
  restoreTrashEntry,
  setRevokedCapabilities,
  stageComponentImport,
  trustStagedComponent,
  type TrashEntry,
} from '@/features/custom-ui/lifecycle-api'
import { customUiQueryKeys } from '@/features/custom-ui/query-keys'
import { componentErrorText } from './error-text'

export function useComponentVersions(id: string, enabled: boolean) {
  return useQuery({
    queryKey: customUiQueryKeys.versions(id),
    queryFn: ({ signal }) => listComponentVersions(id, signal),
    enabled: enabled && Boolean(id),
    retry: false,
    staleTime: 10_000,
    refetchOnWindowFocus: false,
  })
}

export function useComponentUsage(id: string, enabled: boolean) {
  return useQuery({
    queryKey: customUiQueryKeys.usage(id),
    queryFn: ({ signal }) => getComponentUsage(id, signal),
    enabled: enabled && Boolean(id),
    retry: false,
    // 台账是批量落盘的（浏览器 5 秒、Runtime 2 秒一批）：工作室开着时每 15 秒补读一次即可。
    refetchInterval: 15_000,
    refetchOnWindowFocus: false,
  })
}

export function useComponentTrash(enabled: boolean) {
  return useQuery({
    queryKey: customUiQueryKeys.trash(),
    queryFn: ({ signal }) => listComponentTrash(signal),
    enabled,
    retry: false,
    refetchOnWindowFocus: false,
  })
}

export function useStagedDetail(stageId: string) {
  return useQuery({
    queryKey: customUiQueryKeys.staged(stageId),
    queryFn: ({ signal }) => getStagedComponent(stageId, signal),
    enabled: Boolean(stageId),
    retry: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
  })
}

/** 浏览器下载：Blob → 临时链接 → 点击 → 稍后回收（与工作流导出同一做法）。 */
function download(blob: Blob, fileName: string) {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  anchor.click()
  window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export function useLifecycleActions(notify: (message: string) => void) {
  const client = useQueryClient()
  const { t } = useI18n()
  return useMemo(() => {
    const refresh = () => client.invalidateQueries({ queryKey: customUiQueryKeys.all })
    const run = async <T>(task: () => Promise<T>, failure: string): Promise<T | null> => {
      try {
        const result = await task()
        await refresh()
        return result
      } catch (error) {
        const code = componentErrorCode(error)
        notify(code ? `${failure} ${componentErrorText(code, t)}` : failure)
        return null
      }
    }
    return {
      notify,
      restoreVersion: async (row: ComponentRow, n: number) => {
        const result = await run(
          () => restoreComponentVersion(row.id, n),
          t('custom-ui:versions.restoreFailed'),
        )
        if (result) notify(t('custom-ui:versions.restored', { n }))
        return result
      },
      exportZip: async (row: ComponentRow) => {
        try {
          download(await exportComponentZip(row.id), `${row.id}.zip`)
          notify(t('custom-ui:exporting.done', { name: row.name }))
        } catch (error) {
          const code = componentErrorCode(error)
          notify(
            code
              ? `${t('custom-ui:exporting.failed')} ${componentErrorText(code, t)}`
              : t('custom-ui:exporting.failed'),
          )
        }
      },
      stageImport: (file: File) =>
        run(() => stageComponentImport(file), t('custom-ui:importing.stageFailed')),
      trust: async (stageId: string) => {
        const result = await run(
          () => trustStagedComponent(stageId),
          t('custom-ui:importing.trustFailed'),
        )
        if (result)
          notify(
            result.replaced
              ? t('custom-ui:importing.replaced', { name: result.component.name })
              : t('custom-ui:importing.trusted', { name: result.component.name }),
          )
        return result
      },
      discard: async (stageId: string) => {
        const result = await run(
          () => discardStagedComponent(stageId).then(() => true),
          t('custom-ui:importing.discardFailed'),
        )
        if (result) notify(t('custom-ui:importing.discarded'))
        return result
      },
      restoreTrash: async (entry: TrashEntry) => {
        const result = await run(
          () => restoreTrashEntry(entry.entry),
          t('custom-ui:trash.restoreFailed'),
        )
        if (result) notify(t('custom-ui:trash.restored', { name: entry.name }))
        return result
      },
      purgeTrash: async (entry: TrashEntry) => {
        const result = await run(
          () => purgeTrashEntry(entry.entry).then(() => true),
          t('custom-ui:trash.purgeFailed'),
        )
        if (result) notify(t('custom-ui:trash.purged', { name: entry.name }))
        return result
      },
      setRevoked: (row: ComponentRow, revoked: CapabilityType[]) =>
        run(() => setRevokedCapabilities(row.id, revoked), t('custom-ui:usage.revokeFailed')),
    }
  }, [client, notify, t])
}

export type LifecycleActions = ReturnType<typeof useLifecycleActions>
