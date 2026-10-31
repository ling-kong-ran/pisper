// 会话检查器“+”的组件选择器（计划 §4.5 第 3 项）：列出声明了“会话面板”放置、可运行且已启用的组件，
// 每行 [所有会话] [仅此项目]，直接 POST 实例（已有但被关掉、或范围不含本项目的实例改为 PATCH，不建第二个，
// 会话面板每个组件 1 个）。已经在这个会话里显示的组件标“已放置”，点它切到那个标签。
// 目录只在弹层打开时才请求（内容随 Popover 挂载）；文案在懒加载的 custom-ui 命名空间。
import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import type { ComponentInstanceScope, ComponentRow } from '@/features/custom-ui/custom-ui-api'
import {
  componentCatalogQueryOptions,
  componentInstancesQueryOptions,
} from '@/features/custom-ui/queries'
import { sessionScopeMatches } from '@/features/custom-ui/session-scope'
import { canPlace, surfaceDeclared } from '@/features/custom-ui/studio/studio-model'
import { useStudioActions } from '@/features/custom-ui/studio/use-studio-actions'

export type SessionComponentAddMenuProps = {
  cwd: string
  notify(message: string): void
  // 放好（或点了已放置的组件）后：检查器切到这个实例的标签并关掉弹层。
  onAdded(instanceId: string): void
}

function componentName(row: ComponentRow, language: 'zh-CN' | 'en-US') {
  return row.manifest?.i18n[language]?.name || row.name
}

export function SessionComponentAddMenu({ cwd, notify, onAdded }: SessionComponentAddMenuProps) {
  const { t, language } = useI18n()
  const catalog = useQuery(componentCatalogQueryOptions())
  const instancesQuery = useQuery(componentInstancesQueryOptions())
  const actions = useStudioActions(notify)
  const [busy, setBusy] = useState<string | null>(null)
  const instances = instancesQuery.data?.instances ?? []
  const candidates = (catalog.data?.components ?? []).filter(
    (row) => canPlace(row) && surfaceDeclared(row.manifest, 'session-panel'),
  )

  const place = async (row: ComponentRow, scope: ComponentInstanceScope) => {
    if (busy) return
    setBusy(row.id)
    try {
      const result = await actions.place(row, instances, { surface: 'session-panel', scope })
      if (result) onAdded(result.instance.instanceId)
    } finally {
      setBusy(null)
    }
  }

  if (catalog.isPending || instancesQuery.isPending)
    return (
      <p role="status" className="text-content-muted">
        {t('custom-ui:customUiPage.loading')}
      </p>
    )
  if (catalog.error || instancesQuery.error)
    return (
      <div className="flex flex-col items-start gap-2">
        <p role="alert" className="text-content-secondary">
          {t('custom-ui:widget.catalogFailed')}
        </p>
        <Button
          type="button"
          size="sm"
          variant="secondary"
          onClick={() => {
            void catalog.refetch()
            void instancesQuery.refetch()
          }}
        >
          {t('custom-ui:widget.retry')}
        </Button>
      </div>
    )
  if (!candidates.length)
    return <p className="text-content-secondary">{t('custom-ui:sessionPanel.empty')}</p>

  return (
    <div className="flex flex-col gap-2" data-session-component-add-menu>
      <p className="text-content-secondary">{t('custom-ui:sessionPanel.addHint')}</p>
      <ul className="m-0 flex list-none flex-col gap-1 p-0">
        {candidates.map((row) => {
          const name = componentName(row, language)
          const existing = instances.find(
            (instance) => instance.componentId === row.id && instance.surface === 'session-panel',
          )
          const shown = Boolean(existing?.enabled && sessionScopeMatches(existing.scope, cwd))
          return (
            <li
              key={row.id}
              data-session-component-candidate={row.id}
              className="flex flex-col gap-1.5 rounded-sm px-1.5 py-1.5"
            >
              <span className="truncate text-[13px] font-medium text-content">{name}</span>
              {shown && existing ? (
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  className="self-start"
                  onClick={() => onAdded(existing.instanceId)}
                >
                  {t('custom-ui:studio.placed')}
                </Button>
              ) : (
                <div className="flex flex-wrap gap-1.5">
                  <Button
                    type="button"
                    size="sm"
                    variant="secondary"
                    disabled={busy !== null}
                    data-session-component-scope="all"
                    onClick={() => void place(row, { type: 'all' })}
                  >
                    {t('custom-ui:studio.scopeAll')}
                  </Button>
                  <Button
                    type="button"
                    size="sm"
                    variant="secondary"
                    // 没有项目目录的会话不能按项目放。
                    disabled={busy !== null || !cwd}
                    data-session-component-scope="project"
                    onClick={() => void place(row, { type: 'project', cwd })}
                  >
                    {t('custom-ui:sessionPanel.thisProject')}
                  </Button>
                </div>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
