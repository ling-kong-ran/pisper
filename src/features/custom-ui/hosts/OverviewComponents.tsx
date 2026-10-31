// 全部代理的组件卡（overview-card 放置，规格 3.1、3.4、5.5）：桌面 ≥1280 在右侧 320 栏里纵向排列，
// 1080–1279 收进页头“组件 N”弹层，手机排在全部代理列表末尾、全宽。尺寸 S = 内容区 320×120，M = 320×248
// （与工作室预览 1:1，ctx.size 量的就是这个内容区）；卡片头由宿主绘制：名称或实例标题、“组件”标记、⋯
// （设置、在工作室打开、换个位置、移出）。描边用 ring（不占布局），内容区宽度正好等于栏宽。
// 文案只用懒加载的 custom-ui 命名空间；这里经 hosts.ts 的动态边界进入，没有总览实例时永远不会加载。
import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery } from '@tanstack/react-query'
import { ArrowRightLeft, Ellipsis, Plus, Settings2, SquareArrowOutUpRight, X } from 'lucide-react'
import { OVERVIEW_CARD_DIMENSIONS, type ComponentSurface } from '@shared/custom-ui-manifest.mjs'
import { PAGE_PATHS, componentPath } from '@/app/routes'
import { useI18n } from '@/app/use-i18n'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { cn } from '@/lib/utils'
import type { ComponentInstance, ComponentRow } from '@/features/custom-ui/custom-ui-api'
import { ComponentHost } from '@/features/custom-ui/native/ComponentHost'
import { componentInstancesQueryOptions } from '@/features/custom-ui/queries'
import { surfaceLabel } from '@/features/custom-ui/studio/labels'
import { surfaceDeclared } from '@/features/custom-ui/studio/studio-model'
import { useComponentFix } from '@/features/custom-ui/studio/use-component-agent'
import { useStudioActions } from '@/features/custom-ui/studio/use-studio-actions'

type PlacedCard = { instance: ComponentInstance; component: ComponentRow }

export type OverviewComponentsLayout = 'column' | 'popover' | 'phone'

export type OverviewComponentsProps = {
  placed: readonly PlacedCard[]
  layout: OverviewComponentsLayout
  notify(message: string): void
  onOpenSession?(id: string): void
}

// 工作室按路由状态里的实例 id 选中这张卡（属性表单编辑的就是它），见 CustomUiPage。
export type StudioInstanceState = { componentInstanceId: string }

// 换个位置：总览卡片之外的三种放置（总览本身可多实例，“再放一张”在工作室里）。
const MOVE_TARGETS: ComponentSurface[] = ['session-panel', 'feature-page', 'floating']

function cardLabel(placed: PlacedCard, language: 'zh-CN' | 'en-US') {
  const { instance, component } = placed
  return instance.title || component.manifest?.i18n[language]?.name || component.name
}

function CardMenu({
  placed,
  label,
  instances,
  notify,
}: {
  placed: PlacedCard
  label: string
  instances: readonly ComponentInstance[]
  notify(message: string): void
}) {
  const { t } = useI18n()
  const navigate = useNavigate()
  const actions = useStudioActions(notify)
  const [busy, setBusy] = useState(false)
  const { instance, component } = placed

  const moveTo = async (surface: ComponentSurface) => {
    if (busy) return
    setBusy(true)
    try {
      // 先放到新位置，成功后才移出这张卡：中途失败时卡片还在原处，属性不会丢。
      const result = await actions.place(component, instances, {
        surface,
        ...(instance.title ? { title: instance.title } : {}),
        props: instance.props,
      })
      if (!result) return
      if (await actions.unplace(instance))
        notify(t('custom-ui:overview.moved', { place: surfaceLabel(surface, t) }))
    } finally {
      setBusy(false)
    }
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="flex-none"
          disabled={busy}
          aria-label={t('custom-ui:overview.menu', { name: label })}
          title={t('custom-ui:overview.menu', { name: label })}
          data-overview-component-menu
        >
          <Ellipsis aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuItem
          onSelect={() =>
            navigate(componentPath(component.id), {
              state: { componentInstanceId: instance.instanceId } satisfies StudioInstanceState,
            })
          }
        >
          <Settings2 aria-hidden="true" />
          {t('custom-ui:overview.settings')}
        </DropdownMenuItem>
        <DropdownMenuItem onSelect={() => navigate(componentPath(component.id))}>
          <SquareArrowOutUpRight aria-hidden="true" />
          {t('custom-ui:tool.openInStudio')}
        </DropdownMenuItem>
        <DropdownMenuSub>
          <DropdownMenuSubTrigger data-overview-component-move>
            <ArrowRightLeft aria-hidden="true" />
            {t('custom-ui:overview.move')}
          </DropdownMenuSubTrigger>
          <DropdownMenuSubContent className="w-48">
            {MOVE_TARGETS.map((surface) => {
              const declared = surfaceDeclared(component.manifest, surface)
              // 会话面板、功能页、浮窗每个组件各 1 个：已经开着的不能再放；被关掉的会重新打开（不建第二个）。
              const placedThere = instances.some(
                (entry) =>
                  entry.componentId === component.id && entry.surface === surface && entry.enabled,
              )
              return (
                <DropdownMenuItem
                  key={surface}
                  disabled={!declared || placedThere}
                  data-overview-component-move-to={surface}
                  onSelect={() => void moveTo(surface)}
                >
                  {surfaceLabel(surface, t)}
                  <span className="ml-auto text-xs text-content-muted">
                    {!declared
                      ? t('custom-ui:studio.placeUndeclaredShort')
                      : placedThere
                        ? t('custom-ui:studio.placed')
                        : ''}
                  </span>
                </DropdownMenuItem>
              )
            })}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          data-overview-component-remove
          onSelect={() => {
            void actions.unplace(instance).then((result) => {
              if (result) notify(t('custom-ui:overview.removed', { name: label }))
            })
          }}
        >
          <X aria-hidden="true" />
          {t('custom-ui:overview.remove')}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function OverviewCard({
  placed,
  instances,
  notify,
  onOpenSession,
}: {
  placed: PlacedCard
  instances: readonly ComponentInstance[]
  notify(message: string): void
  onOpenSession?(id: string): void
}) {
  const { t, language } = useI18n()
  const { instance, component } = placed
  const size = instance.size === 'M' ? 'M' : 'S'
  const label = cardLabel(placed, language)
  const fix = useComponentFix(component, notify)
  return (
    <article
      aria-label={label}
      data-overview-component={instance.instanceId}
      data-overview-component-id={component.id}
      data-overview-component-size={size}
      className="overflow-hidden rounded-[var(--r-lg)] bg-panel ring-1 ring-stroke"
    >
      <header className="flex h-9 items-center gap-2 border-b border-stroke-soft pr-1 pl-3 pointer-coarse:h-12">
        <h3 className="min-w-0 flex-1 truncate text-[13px] leading-5 font-medium text-content">
          {label}
        </h3>
        <span className="flex-none rounded-xs border border-stroke px-1.5 text-[11px] leading-4 text-content-muted">
          {t('custom-ui:overview.badge')}
        </span>
        <CardMenu placed={placed} label={label} instances={instances} notify={notify} />
      </header>
      {/* 内容区固定高度（S 120 / M 248），宽度随栏宽（桌面正好 320）；组件只负责这一块，溢出裁掉。 */}
      <div
        data-overview-component-body
        className="overflow-hidden"
        style={{ height: OVERVIEW_CARD_DIMENSIONS[size].h }}
      >
        <ComponentHost
          component={component}
          instance={{
            instanceId: instance.instanceId,
            surface: 'overview-card',
            title: instance.title,
            props: instance.props,
            size,
          }}
          notify={notify}
          onOpenSession={onOpenSession}
          onRequestFix={fix}
          className="h-full"
        />
      </div>
    </article>
  )
}

// 列末的“+ 放置组件”：去组件工作室（放置都在那里做，规格 5.5 不偏向任何一处）。
function PlaceLink({ className }: { className?: string }) {
  const { t } = useI18n()
  const navigate = useNavigate()
  return (
    <button
      type="button"
      data-overview-component-place
      onClick={() => navigate(PAGE_PATHS.components)}
      className={cn(
        'flex h-9 w-full items-center justify-center gap-1.5 rounded-[var(--r-lg)] border border-dashed border-stroke-strong text-[12.5px] text-content-muted transition-colors duration-(--d-fast) hover:bg-surface-hover hover:text-content focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-focus pointer-coarse:h-11',
        className,
      )}
    >
      <Plus className="size-3.5" aria-hidden="true" />
      {t('custom-ui:overview.place')}
    </button>
  )
}

export function OverviewComponents({
  placed,
  layout,
  notify,
  onOpenSession,
}: OverviewComponentsProps) {
  const { t } = useI18n()
  // 全部实例（含别处的）：“换个位置”据此判断目标位置是否已经放了这个组件。与放置入口共用同一份缓存。
  const instances = useQuery(componentInstancesQueryOptions()).data?.instances ?? []
  const cards = placed.map((entry) => (
    <OverviewCard
      key={entry.instance.instanceId}
      placed={entry}
      instances={instances}
      notify={notify}
      onOpenSession={onOpenSession}
    />
  ))

  if (layout === 'popover')
    return (
      <Popover>
        <PopoverTrigger asChild>
          <Button variant="outline" data-overview-components-trigger>
            {t('custom-ui:overview.trigger', { count: placed.length })}
          </Button>
        </PopoverTrigger>
        <PopoverContent
          align="end"
          aria-label={t('custom-ui:overview.heading')}
          data-overview-components="popover"
          // 320 的卡片 + 两侧 12：与右栏同宽；高度不超过可用空间，多张卡片在弹层里滚动。
          className="max-h-(--radix-popover-content-available-height) w-[344px] gap-3 overflow-y-auto p-3"
        >
          {cards}
          <PlaceLink />
        </PopoverContent>
      </Popover>
    )

  return (
    <div
      data-overview-components={layout}
      className={cn('flex flex-col gap-3', layout === 'phone' && 'mt-6')}
    >
      <h2 className="text-[13px] leading-5 font-medium text-content-secondary">
        {t('custom-ui:overview.heading')}
      </h2>
      {cards}
      <PlaceLink />
    </div>
  )
}
