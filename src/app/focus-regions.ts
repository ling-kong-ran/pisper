// F6 焦点区域切换（规格 v2 第 2.8 节）：侧栏 → 顶栏 → 对话（或页面内容）→ 输入框，循环。
// 区域由各处的 data-focus-region 标记声明，外壳不必知道每个页面的内部结构；没有可聚焦元素的区域
// （例如新任务草稿只有标题的顶栏）直接跳过。按需动态导入：只在第一次按 F6 时加载，不占入口包。
// 主区 <main> 标为 main，只在页面没有自己的“content”区域（会话转录）时充当内容区。

type RegionKind = 'sidebar' | 'header' | 'content' | 'main' | 'composer'

const RANK: Record<RegionKind, number> = { sidebar: 0, header: 1, content: 2, main: 2, composer: 3 }

const TABBABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

function visible(element: Element) {
  if (!(element instanceof HTMLElement)) return false
  if (element.closest('[inert], [aria-hidden="true"]')) return false
  return element.getClientRects().length > 0 && getComputedStyle(element).visibility !== 'hidden'
}

type Region = { kind: RegionKind; element: HTMLElement }

function regionKind(element: Element): RegionKind | null {
  const kind = element.getAttribute('data-focus-region')
  return kind && kind in RANK ? (kind as RegionKind) : null
}

export function focusRegions(root: ParentNode = document): Region[] {
  const all = [...root.querySelectorAll<HTMLElement>('[data-focus-region]')]
    .map((element) => ({ kind: regionKind(element), element }))
    .filter((region): region is Region => region.kind !== null && visible(region.element))
  const hasContent = all.some((region) => region.kind === 'content')
  // 稳定排序：同级区域保持文档顺序（例如侧聊另有一个输入框时排在主输入框之后）。
  return all
    .filter((region) => !(region.kind === 'main' && hasContent))
    .map((region, index) => ({ region, index }))
    .sort((a, b) => RANK[a.region.kind] - RANK[b.region.kind] || a.index - b.index)
    .map(({ region }) => region)
}

// 区域里该落焦点的元素：侧栏优先当前会话行，其次第一行，再其次第一个入口；输入框区优先文本框；
// 主区回退时跳过嵌套在顶栏等其他区域里的元素，避免 F6 两次落在同一个按钮上。
export function regionTarget({ kind, element }: Region): HTMLElement | null {
  const scope =
    kind === 'sidebar'
      ? (element.querySelector<HTMLElement>('[data-shell-sidebar-peek]') ?? element)
      : element
  const candidates = (selector: string) =>
    [...scope.querySelectorAll<HTMLElement>(selector)].filter(
      (candidate) =>
        visible(candidate) &&
        (kind !== 'main' || candidate.closest('[data-focus-region]') === element),
    )
  if (kind === 'sidebar') {
    const current = candidates('[data-sidebar-row][aria-current="page"]')[0]
    if (current) return current
    const row = candidates('[data-sidebar-row]')[0]
    if (row) return row
  }
  if (kind === 'composer') {
    const field = candidates('textarea:not([disabled])')[0]
    if (field) return field
  }
  return candidates(TABBABLE)[0] ?? null
}

// 当前焦点所在的区域：嵌套时取最内层（主区里的顶栏算顶栏）。
function currentIndex(regions: Region[], active: Element | null) {
  if (!active || active === document.body) return -1
  let best = -1
  regions.forEach((region, index) => {
    if (!region.element.contains(active)) return
    if (best < 0 || regions[best].element.contains(region.element)) best = index
  })
  return best
}

export function cycleFocusRegion() {
  const regions = focusRegions()
  const start = currentIndex(regions, document.activeElement)
  for (let step = 1; step <= regions.length; step += 1) {
    const target = regionTarget(regions[(start + step) % regions.length])
    if (!target || target === document.activeElement) continue
    target.focus()
    target.scrollIntoView?.({ block: 'nearest' })
    return true
  }
  return false
}
