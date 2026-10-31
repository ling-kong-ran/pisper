// 组件工作室的纯逻辑（规格 5.1、5.5，计划 §4.4）：列表可见行、筛选与搜索、放置摘要、预览档位与舞台尺寸。
// 不依赖 React，便于单测；文案由界面按返回的描述翻译。
import {
  FLOATING_DEFAULT_SIZE,
  OVERVIEW_CARD_DIMENSIONS,
  type ComponentSurface,
  type NativeManifest,
  type OverviewCardSize,
} from '@shared/custom-ui-manifest.mjs'
import type { ComponentInstance, ComponentRow } from '@/features/custom-ui/custom-ui-api'

// 会话检查器宽度（规格 5.5：456，最小窗口 1080 时 420）。
export const SESSION_PANEL_WIDTH = { standard: 456, min: 420 } as const
// 最小窗口 1080 时功能页主区宽度：1080 − 侧栏 268。
export const FEATURE_PAGE_MIN_WIDTH = 812
// 右侧设置栏能并排放下的最小页面宽度：组件库 284 + 工作台至少 704 + 设置 372。
// 1440 窗口 + 60 窄轨约 1380（主区的描边、滚动条会吃掉一两个像素，所以留 20 的余量）；
// 侧栏展开（1440 − 268 = 1172）或窗口更窄时设置栏改为抽屉（规格 5.1）。
export const STUDIO_INLINE_SETTINGS_MIN = 1360

export type StudioFilter = 'all' | 'enabled' | 'attention'
export type PreviewTarget =
  'overview-S' | 'overview-M' | 'session-panel' | 'feature-page' | 'floating'
export type PreviewWindow = 'standard' | 'min'

/** 旧 iframe 组件（legacy）不展示：启动迁移已把它们移进回收站（负责人决定）。 */
export function visibleStudioRows(rows: readonly ComponentRow[]): ComponentRow[] {
  return rows.filter((row) => row.kind !== 'legacy')
}

export function componentDisplayName(row: ComponentRow, language: string): string {
  const i18n = row.manifest?.i18n as Record<string, { name?: string } | undefined> | undefined
  return i18n?.[language]?.name || row.name || row.id
}

export function componentDisplayDescription(row: ComponentRow, language: string): string {
  const i18n = row.manifest?.i18n as
    Record<string, { description?: string } | undefined> | undefined
  return i18n?.[language]?.description || row.description || ''
}

/** 需要处理：manifest 有错、目录无法读取，或者本页里崩溃过 / 被暂停（troubled 由熔断状态给出）。 */
export function rowNeedsAttention(row: ComponentRow, troubled: ReadonlySet<string>): boolean {
  return row.kind === 'broken' || row.status !== 'ready' || troubled.has(row.id)
}

export function rowIsEnabled(row: ComponentRow): boolean {
  return row.kind === 'native' && row.enabled
}

export function filterStudioRows(
  rows: readonly ComponentRow[],
  {
    filter,
    query,
    language,
    troubled,
  }: { filter: StudioFilter; query: string; language: string; troubled: ReadonlySet<string> },
): ComponentRow[] {
  const needle = query.trim().toLocaleLowerCase()
  return rows.filter((row) => {
    if (filter === 'enabled' && !rowIsEnabled(row)) return false
    if (filter === 'attention' && !rowNeedsAttention(row, troubled)) return false
    if (!needle) return true
    return [componentDisplayName(row, language), row.id, componentDisplayDescription(row, language)]
      .join('\n')
      .toLocaleLowerCase()
      .includes(needle)
  })
}

export function studioFilterCounts(rows: readonly ComponentRow[], troubled: ReadonlySet<string>) {
  return {
    all: rows.length,
    enabled: rows.filter(rowIsEnabled).length,
    attention: rows.filter((row) => rowNeedsAttention(row, troubled)).length,
  }
}

export type PlacementSummary = {
  surface: ComponentSurface
  sizes: OverviewCardSize[]
  count: number
}

const SURFACE_ORDER: readonly ComponentSurface[] = [
  'overview-card',
  'session-panel',
  'feature-page',
  'floating',
]

/** 行内放置摘要（“总览 M · 会话面板”）：只算已启用的实例，按固定顺序；总览列出出现过的尺寸与张数。 */
export function placementSummary(
  componentId: string,
  instances: readonly ComponentInstance[],
): PlacementSummary[] {
  const result: PlacementSummary[] = []
  for (const surface of SURFACE_ORDER) {
    const own = instances.filter(
      (instance) =>
        instance.componentId === componentId && instance.surface === surface && instance.enabled,
    )
    if (!own.length) continue
    const sizes = (['S', 'M'] as const).filter((size) => own.some((item) => item.size === size))
    result.push({ surface, sizes: surface === 'overview-card' ? sizes : [], count: own.length })
  }
  return result
}

/** manifest 声明的放置 → 预览档位（总览按声明的尺寸拆成 S / M）。 */
export function previewTargets(manifest: NativeManifest | undefined): PreviewTarget[] {
  if (!manifest) return []
  const targets: PreviewTarget[] = []
  for (const surface of SURFACE_ORDER) {
    const placement = manifest.placements.find((entry) => entry.surface === surface)
    if (!placement) continue
    if (surface === 'overview-card') {
      const sizes = placement.sizes?.length ? placement.sizes : (['S', 'M'] as const)
      for (const size of sizes) targets.push(size === 'S' ? 'overview-S' : 'overview-M')
    } else targets.push(surface)
  }
  return targets
}

export function targetSurface(target: PreviewTarget): ComponentSurface {
  return target === 'overview-S' || target === 'overview-M' ? 'overview-card' : target
}

export function targetCardSize(target: PreviewTarget): OverviewCardSize | null {
  return target === 'overview-S' ? 'S' : target === 'overview-M' ? 'M' : null
}

/** 某个实例对应的预览档位（选中实例时预览跟着切过去）。 */
export function instanceTarget(
  instance: Pick<ComponentInstance, 'surface' | 'size'>,
): PreviewTarget {
  if (instance.surface === 'overview-card')
    return instance.size === 'M' ? 'overview-M' : 'overview-S'
  return instance.surface
}

// 舞台里的框：数字是 1:1 的像素，'fill' 表示随舞台宽 / 高。
export type PreviewFrame = { width: number | 'fill'; height: number | 'fill'; label: string }

export function previewFrame(
  target: PreviewTarget,
  manifest: NativeManifest | undefined,
  window: PreviewWindow,
): PreviewFrame {
  if (target === 'overview-S' || target === 'overview-M') {
    const size = target === 'overview-S' ? 'S' : 'M'
    const frame = OVERVIEW_CARD_DIMENSIONS[size]
    return { width: frame.w, height: frame.h, label: `${size} · ${frame.w}×${frame.h}` }
  }
  if (target === 'session-panel') {
    const width = SESSION_PANEL_WIDTH[window]
    return { width, height: 'fill', label: `${width}` }
  }
  if (target === 'floating') {
    const size =
      manifest?.placements.find((entry) => entry.surface === 'floating')?.size ??
      FLOATING_DEFAULT_SIZE
    return { width: size.w, height: size.h, label: `${size.w}×${size.h}` }
  }
  return window === 'min'
    ? { width: FEATURE_PAGE_MIN_WIDTH, height: 'fill', label: `${FEATURE_PAGE_MIN_WIDTH}` }
    : { width: 'fill', height: 'fill', label: '' }
}

/** 某个放置在 manifest 里是否声明（没声明的放置不能放，磁贴与菜单项置灰）。 */
export function surfaceDeclared(
  manifest: NativeManifest | undefined,
  surface: ComponentSurface,
): boolean {
  return Boolean(manifest?.placements.some((entry) => entry.surface === surface))
}

/** 新建总览卡片默认用的尺寸：manifest 声明的第一个，缺省 S。 */
export function defaultCardSize(manifest: NativeManifest | undefined): OverviewCardSize {
  return manifest?.placements.find((entry) => entry.surface === 'overview-card')?.sizes?.[0] ?? 'S'
}

/** 能放置：原生、可运行、已启用（Runtime 创建实例的前提，计划 §3.3）。 */
export function canPlace(row: ComponentRow): boolean {
  return row.kind === 'native' && row.status === 'ready' && row.enabled && Boolean(row.manifest)
}

/** 新选中的组件：路由给的 id 存在就用它，否则上次在本页选过的，再否则第一行。 */
export function resolveSelection(
  rows: readonly ComponentRow[],
  requested: string,
  remembered: string,
): ComponentRow | null {
  return (
    rows.find((row) => row.id === requested) ||
    (!requested ? rows.find((row) => row.id === remembered) : undefined) ||
    (!requested ? rows[0] : undefined) ||
    null
  )
}

// 组件名称上限（与 Runtime 的模板创建一致：1–120）。
export const COMPONENT_NAME_MAX = 120
