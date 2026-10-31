import { PageFrame } from '@/components/app/PageFrame'
// 组件工作室（规格 5.1，A7 S5）：桌面三栏——组件库 284 | 工作台 | 设置 372。进入时应用侧栏收成窄轨
// （只在本次停留期间，不改 'pisper-sidebar' 偏好）；页面宽度放不下三栏（窗口 <1440 或用户展开侧栏）时
// 右侧设置栏改为抽屉。手机外壳（规格 7.4）：列表 → 详情两级，详情依次是预览、属性、能力、放置。
// 选中项由路由 /components/:id 承载；筛选、预览档位、选中实例等界面状态只在内存里（计划 §4.7 不新增偏好键）。
// 旧 iframe 组件已删除：启动迁移把它们移进回收站，迁移后手放的旧格式目录这里不展示（负责人决定）。
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { useLocation } from 'react-router-dom'
import { ChevronLeft, X } from 'lucide-react'
import { useI18n } from '@/app/use-i18n'
import { InlineEmptyState, PageState } from '@/components/app/PageState'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet'
import { usePhoneShell } from '@/hooks/usePhoneShell'
import { useSidebarStore } from '@/stores/sidebar-store'
import type { Notify } from '@/app/route-context'
import type { ComponentInstance, ComponentRow } from './custom-ui-api'
import { componentCircuitBreaker } from './native/host-runtime'
import { useElementBox } from './native/size'
import { useComponentCatalog, useComponentInstanceList, useComponentsAvailable } from './placements'
import { createCommitStore } from './studio/commit-store'
import { PreviewBar, PreviewStage } from './studio/PreviewStage'
import type { PreviewDraft } from './studio/PropsForm'
import { StudioImportButton, StudioLibrary, StudioRow } from './studio/StudioLibrary'
import {
  CapabilitiesSection,
  DeleteComponentButton,
  ErrorList,
  IdentityRow,
  PlacementSection,
  PropsSection,
  StudioSettings,
} from './studio/StudioSettings'
import { SettingsDrawerButton, StudioWorkbench, type WorkbenchTab } from './studio/StudioWorkbench'
import { NewComponentMenu } from './studio/NewComponentMenu'
import {
  STUDIO_INLINE_SETTINGS_MIN,
  componentDisplayName,
  filterStudioRows,
  instanceTarget,
  previewTargets,
  resolveSelection,
  studioFilterCounts,
  visibleStudioRows,
  type PreviewTarget,
  type PreviewWindow,
  type StudioFilter,
} from './studio/studio-model'
import { useStudioActions } from './studio/use-studio-actions'

type CustomUiPageProps = {
  notify: Notify
  // 路由里的选中组件（/components/:id）；空串表示没有指定。
  selectedId?: string
  // 选中或回到列表（id 为空）；replace 为真时替换当前历史记录（桌面三栏内切换）。
  onSelect: (id: string, replace: boolean) => void
}

// 本页上次选中的组件：不带 id 回到工作室时沿用（只在内存里，刷新即忘）。
let rememberedSelection = ''

const EMPTY_INSTANCES: ComponentInstance[] = []

/**
 * 桌面工作室的外框：量页面宽度决定设置栏并排还是放进抽屉（规格 5.1）。
 * 单独成组件是为了让量尺寸的元素随它一起挂载（页面先渲染加载态时，量尺寸的 effect 不会落空）。
 */
function StudioFrame({ children }: { children: (inline: boolean) => ReactNode }) {
  const ref = useRef<HTMLDivElement>(null)
  const box = useElementBox(ref)
  const inline = box.measured && box.width >= STUDIO_INLINE_SETTINGS_MIN
  return (
    <PageFrame variant="workbench">
      <div
        ref={ref}
        data-component-studio
        data-studio-settings-mode={inline ? 'inline' : 'drawer'}
        className="h-full min-h-0"
      >
        {children(inline)}
      </div>
    </PageFrame>
  )
}

/** 进入工作室时应用侧栏收成窄轨，离开时回到用户偏好（规格 2.3 / 5.1）。 */
function useStudioRail(enabled: boolean) {
  useLayoutEffect(() => {
    if (!enabled) return
    const sidebar = useSidebarStore.getState()
    sidebar.enterRouteRail()
    return () => useSidebarStore.getState().leaveRouteRail()
  }, [enabled])
}

/** 本页里崩溃过或被暂停的组件（预览实例或它的任一放置实例）：“需处理”筛选与行内错误条。 */
function useTroubledComponents(
  rows: readonly ComponentRow[],
  instances: readonly ComponentInstance[],
): ReadonlySet<string> {
  const breaker = componentCircuitBreaker()
  const [version, setVersion] = useState(() => breaker.version())
  useEffect(() => breaker.subscribe(() => setVersion(breaker.version())), [breaker])
  // 组件文件变了（热重载，新 rev）就清掉预览实例的崩溃记录：新代码重新计数。
  const revs = useRef(new Map<string, string>())
  useEffect(() => {
    for (const row of rows) {
      const previous = revs.current.get(row.id)
      if (previous && row.rev && previous !== row.rev) breaker.resume(`preview:${row.id}`)
      if (row.rev) revs.current.set(row.id, row.rev)
    }
  }, [breaker, rows])
  return useMemo(() => {
    void version
    const troubled = new Set<string>()
    for (const row of rows) {
      if (
        breaker.hasTrouble(`preview:${row.id}`) ||
        instances.some(
          (instance) => instance.componentId === row.id && breaker.hasTrouble(instance.instanceId),
        )
      )
        troubled.add(row.id)
    }
    return troubled
  }, [breaker, instances, rows, version])
}

/** 路由状态里指定的放置实例（全部代理组件卡 ⋯ → 设置）：工作室打开时选中它，属性表单编辑的就是这张卡。 */
function requestedInstanceId(state: unknown): string {
  if (typeof state !== 'object' || state === null) return ''
  const value = (state as { componentInstanceId?: unknown }).componentInstanceId
  return typeof value === 'string' && value.length <= 64 ? value : ''
}

/** 预览档位、选中实例与属性草稿：随选中的组件切换，草稿按“组件 + 实例”分键，换了就回到已保存的值。 */
function usePreviewState(
  row: ComponentRow | null,
  instances: readonly ComponentInstance[],
  requestedInstance: string,
) {
  const [targetState, setTargetState] = useState<{ id: string; target: PreviewTarget } | null>(null)
  const [instanceState, setInstanceState] = useState<{ id: string; instanceId: string | null }>({
    id: '',
    instanceId: null,
  })
  const [window, setWindow] = useState<PreviewWindow>('standard')
  const rowId = row?.id ?? ''
  // 带着实例进来（或换了一张卡）时选中它；预览档位随之回到这个实例所在的放置。
  useEffect(() => {
    if (!rowId || !requestedInstance) return
    setInstanceState({ id: rowId, instanceId: requestedInstance })
    setTargetState(null)
  }, [requestedInstance, rowId])
  const targets = previewTargets(row?.manifest)
  const own = row ? instances.filter((instance) => instance.componentId === row.id) : []
  // 用户明确选了“预览属性”（null）就保持；选过的实例被删了，或还没选过，回到第一个放置。
  const chosen = row && instanceState.id === row.id ? instanceState.instanceId : undefined
  const instanceId =
    chosen === null
      ? null
      : chosen && own.some((item) => item.instanceId === chosen)
        ? chosen
        : (own[0]?.instanceId ?? null)
  const instance = own.find((item) => item.instanceId === instanceId) ?? null
  const target =
    row && targetState?.id === row.id && targets.includes(targetState.target)
      ? targetState.target
      : instance && targets.includes(instanceTarget(instance))
        ? instanceTarget(instance)
        : (targets[0] ?? 'feature-page')
  const draftKey = `${row?.id ?? ''}:${instanceId ?? 'preview'}`
  const [draft, setDraft] = useState<PreviewDraft & { key: string }>({
    key: '',
    title: '',
    props: {},
  })
  const onPreview = useCallback(
    (next: PreviewDraft) => setDraft({ key: draftKey, ...next }),
    [draftKey],
  )
  const effectiveDraft =
    draft.key === draftKey ? draft : { title: instance?.title ?? '', props: instance?.props ?? {} }
  return {
    target,
    window,
    instanceId,
    draft: effectiveDraft,
    onPreview,
    setWindow,
    setTarget: (next: PreviewTarget) => row && setTargetState({ id: row.id, target: next }),
    selectInstance: (next: string | null) => {
      if (!row) return
      setInstanceState({ id: row.id, instanceId: next })
      const picked = own.find((item) => item.instanceId === next)
      if (picked && targets.includes(instanceTarget(picked)))
        setTargetState({ id: row.id, target: instanceTarget(picked) })
    },
  }
}

export function CustomUiPage({
  notify,
  selectedId: routeSelectedId = '',
  onSelect,
}: CustomUiPageProps) {
  const { t, language } = useI18n()
  const phone = usePhoneShell()
  const available = useComponentsAvailable()
  const catalog = useComponentCatalog()
  const instanceList = useComponentInstanceList()
  const instances = instanceList.data?.instances ?? EMPTY_INSTANCES
  const actions = useStudioActions(notify)
  const rows = useMemo(() => visibleStudioRows(catalog.data?.components ?? []), [catalog.data])
  const root = catalog.data?.root || ''
  const canDelete = catalog.data?.features.trash === 'move'
  const [filter, setFilter] = useState<StudioFilter>('all')
  const [query, setQuery] = useState('')
  const [tab, setTab] = useState<WorkbenchTab>('preview')
  const [drawerOpen, setDrawerOpen] = useState(false)
  const [reloadToken, setReloadToken] = useState(0)
  const [commits] = useState(createCommitStore)
  const troubled = useTroubledComponents(rows, instances)
  const selected = phone
    ? (rows.find((row) => row.id === routeSelectedId) ?? null)
    : resolveSelection(rows, routeSelectedId, rememberedSelection)
  const selectedId = selected?.id ?? ''
  useEffect(() => {
    if (selectedId) rememberedSelection = selectedId
  }, [selectedId])
  const location = useLocation()
  const preview = usePreviewState(selected, instances, requestedInstanceId(location.state))
  useStudioRail(!phone && available && !catalog.data?.legacyShape)

  const select = (id: string) => {
    setTab('preview')
    onSelect(id, true)
  }
  const create = async (
    template: Parameters<typeof actions.createFromTemplate>[0],
    name?: string,
  ) => {
    const row = await actions.createFromTemplate(template, name)
    if (row) {
      setFilter('all')
      setQuery('')
      onSelect(row.id, !phone)
      notify(t('custom-ui:studio.created', { name: componentDisplayName(row, language) }))
    }
    return row
  }

  // 旧 Runtime（没有 components.native，或列表是旧形状）：如实说明不支持，不展示旧组件当成可用。
  if (!available || catalog.data?.legacyShape)
    return (
      <PageFrame variant="collection">
        <PageState
          kind="empty"
          title={t('custom-ui:studio.unsupportedTitle')}
          description={t('custom-ui:studio.unsupportedHint')}
        />
      </PageFrame>
    )
  if (catalog.isPending)
    return <PageState kind="loading" title={t('custom-ui:customUiPage.loading')} />
  if (catalog.error && !catalog.data)
    return (
      <PageFrame variant="collection">
        <PageState
          kind="error"
          title={t('custom-ui:widget.catalogFailed')}
          action={
            <Button variant="secondary" size="sm" onClick={() => void catalog.refetch()}>
              {t('custom-ui:widget.retry')}
            </Button>
          }
        />
      </PageFrame>
    )

  const counts = studioFilterCounts(rows, troubled)
  const filtered = filterStudioRows(rows, { filter, query, language, troubled })
  const emptyHint = (
    <>
      {t('custom-ui:customUiPage.emptyHint')}
      {root && (
        <code className="mx-auto mt-2 block max-w-full rounded-xs bg-inline-code px-2 py-1 font-mono text-xs break-all text-content-secondary">
          {root}
        </code>
      )}
      <span className="mt-2 block">{t('custom-ui:customUiPage.directoryLocationHint')}</span>
    </>
  )

  // —— 手机：列表与详情各占一屏 ——
  if (phone) {
    const backToList = () => onSelect('', false)
    if (routeSelectedId)
      return (
        <PageFrame variant="collection">
          <div data-components-phone="detail" className="flex min-w-0 flex-col gap-4">
            <Button
              variant="ghost"
              className="-ml-2 min-h-11 w-fit px-2 text-content-secondary"
              onClick={backToList}
            >
              <ChevronLeft aria-hidden="true" />
              {t('custom-ui:phone.allComponents')}
            </Button>
            {selected ? (
              <div
                data-component-stage={selected.id}
                data-component-kind={selected.kind}
                className="flex min-w-0 flex-col gap-5"
              >
                <IdentityRow row={selected} actions={actions} />
                <ErrorList row={selected} />
                {selected.kind === 'native' && selected.status === 'ready' && selected.manifest && (
                  <>
                    <section className="flex flex-col gap-2">
                      <PreviewBar
                        manifest={selected.manifest}
                        target={preview.target}
                        window="standard"
                        phone
                        onTarget={preview.setTarget}
                        onWindow={preview.setWindow}
                      />
                      <PreviewStage
                        key={reloadToken}
                        row={selected}
                        target={preview.target}
                        window="standard"
                        instanceId={preview.instanceId}
                        title={preview.draft.title}
                        props={preview.draft.props}
                        phone
                        notify={notify}
                        commits={commits}
                      />
                    </section>
                    <PropsSection
                      row={selected}
                      instances={instances}
                      selectedInstanceId={preview.instanceId}
                      notify={notify}
                      actions={actions}
                      onSelectInstance={preview.selectInstance}
                      onPreview={preview.onPreview}
                    />
                    <CapabilitiesSection row={selected} />
                    <PlacementSection row={selected} instances={instances} actions={actions} />
                  </>
                )}
                {canDelete && (
                  <DeleteComponentButton
                    row={selected}
                    actions={actions}
                    variant="button"
                    onDeleted={backToList}
                  />
                )}
              </div>
            ) : (
              <PageState
                kind="empty"
                title={t('custom-ui:phone.missingTitle')}
                description={t('custom-ui:phone.missingHint')}
                action={
                  <Button variant="secondary" className="min-h-11" onClick={backToList}>
                    {t('custom-ui:phone.allComponents')}
                  </Button>
                }
              />
            )}
          </div>
        </PageFrame>
      )
    return (
      <PageFrame variant="collection">
        <div data-components-phone="list" className="flex min-w-0 flex-col gap-3">
          <div className="flex items-center gap-2">
            <span className="mr-auto text-sm font-medium text-content-secondary tabular-nums">
              {t('custom-ui:customUiPage.installedComponents', { count: rows.length })}
            </span>
            <StudioImportButton className="min-h-11" />
            <NewComponentMenu onCreate={create} className="min-h-11" />
          </div>
          {rows.length ? (
            <ul className="flex flex-col divide-y divide-stroke overflow-hidden rounded-[var(--r-lg)] border border-stroke bg-panel">
              {rows.map((row) => (
                <li key={row.id}>
                  <StudioRow
                    row={row}
                    instances={instances}
                    active={false}
                    phone
                    onSelect={() => onSelect(row.id, false)}
                  />
                </li>
              ))}
            </ul>
          ) : (
            <InlineEmptyState
              title={t('custom-ui:customUiPage.emptyTitle')}
              description={emptyHint}
            />
          )}
          <p className="text-xs leading-relaxed break-all text-content-muted">
            {t('custom-ui:customUiPage.directoryHint', { path: root })}
          </p>
        </div>
      </PageFrame>
    )
  }

  // —— 桌面三栏 ——
  const settings = selected && (
    <StudioSettings
      key={selected.id}
      row={selected}
      instances={instances}
      selectedInstanceId={preview.instanceId}
      canDelete={canDelete}
      notify={notify}
      actions={actions}
      onSelectInstance={preview.selectInstance}
      onPreview={preview.onPreview}
      onViewSource={() => {
        setTab('source')
        setDrawerOpen(false)
      }}
      onDeleted={() => onSelect('', true)}
    />
  )
  return (
    <StudioFrame>
      {(inline) => (
        <>
          <div
            data-studio-columns
            className={
              inline
                ? 'grid h-full min-h-0 grid-cols-[284px_minmax(0,1fr)_372px]'
                : 'grid h-full min-h-0 grid-cols-[284px_minmax(0,1fr)]'
            }
          >
            <StudioLibrary
              rows={filtered}
              allCount={rows.length}
              counts={counts}
              instances={instances}
              selectedId={selected?.id ?? ''}
              troubled={troubled}
              filter={filter}
              query={query}
              root={root}
              onFilter={setFilter}
              onQuery={setQuery}
              onSelect={select}
              onCreate={create}
              onOpenLogs={(id) => {
                onSelect(id, true)
                setTab('logs')
              }}
              onReload={(id) => {
                componentCircuitBreaker().resume(`preview:${id}`)
                setReloadToken((value) => value + 1)
                setTab('preview')
                onSelect(id, true)
              }}
            />
            {selected ? (
              <StudioWorkbench
                key={`${selected.id}#${reloadToken}`}
                row={selected}
                tab={tab}
                target={preview.target}
                window={preview.window}
                instanceId={preview.instanceId}
                draft={preview.draft}
                notify={notify}
                commits={commits}
                settingsButton={
                  !inline && <SettingsDrawerButton onOpen={() => setDrawerOpen(true)} />
                }
                onTab={setTab}
                onTarget={preview.setTarget}
                onWindow={preview.setWindow}
              />
            ) : (
              <InlineEmptyState
                className="min-h-0"
                title={
                  rows.length
                    ? t('custom-ui:phone.missingTitle')
                    : t('custom-ui:customUiPage.emptyTitle')
                }
                description={rows.length ? t('custom-ui:phone.missingHint') : emptyHint}
              />
            )}
            {inline && selected && (
              <aside
                aria-label={t('custom-ui:studio.settings')}
                className="flex min-h-0 min-w-0 flex-col border-l border-stroke"
              >
                {settings}
              </aside>
            )}
          </div>
          {!inline && (
            <Sheet open={drawerOpen && Boolean(selected)} onOpenChange={setDrawerOpen}>
              <SheetContent
                side="right"
                showCloseButton={false}
                data-studio-settings-drawer
                className="w-[372px] max-w-[calc(100vw-48px)] gap-0 p-0 sm:max-w-[372px]"
              >
                <SheetHeader className="flex-row items-center justify-between border-b border-stroke px-4 py-2.5">
                  <SheetTitle className="text-sm">{t('custom-ui:studio.settings')}</SheetTitle>
                  <SheetDescription className="sr-only">
                    {t('custom-ui:studio.settingsHint')}
                  </SheetDescription>
                  <Button
                    size="icon-sm"
                    variant="ghost"
                    aria-label={t('custom-ui:studio.closeSettings')}
                    onClick={() => setDrawerOpen(false)}
                  >
                    <X />
                  </Button>
                </SheetHeader>
                <div className="min-h-0 flex-1">{settings}</div>
              </SheetContent>
            </Sheet>
          )}
        </>
      )}
    </StudioFrame>
  )
}
