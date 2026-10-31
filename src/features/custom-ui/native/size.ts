// 尺寸契约（规格 5.5）：宿主经 ctx.size 实时传入 {units, width, height, compact}；宽度 < 300 时 compact。
// 总览卡 S 记 1 个单位、M 记 2 个；其他放置都是 1。
import { useLayoutEffect, useState, type RefObject } from 'react'
import type { ComponentSurface, OverviewCardSize } from '@shared/custom-ui-manifest.mjs'

export const COMPACT_WIDTH = 300

export type ComponentSize = { units: number; width: number; height: number; compact: boolean }

export function computeComponentSize({
  surface,
  cardSize,
  width,
  height,
}: {
  surface: ComponentSurface
  cardSize?: OverviewCardSize | null
  width: number
  height: number
}): ComponentSize {
  const w = Number.isFinite(width) && width > 0 ? Math.round(width) : 0
  const h = Number.isFinite(height) && height > 0 ? Math.round(height) : 0
  return {
    units: surface === 'overview-card' && cardSize === 'M' ? 2 : 1,
    width: w,
    height: h,
    compact: w < COMPACT_WIDTH,
  }
}

export function sameComponentSize(left: ComponentSize, right: ComponentSize) {
  return (
    left.units === right.units &&
    left.width === right.width &&
    left.height === right.height &&
    left.compact === right.compact
  )
}

/** 观察容器尺寸；首次在 layout effect 里同步量一次，measured 之前不渲染组件（避免先按 0 宽渲染一帧）。 */
export function useElementBox(ref: RefObject<HTMLElement | null>) {
  const [box, setBox] = useState<{ width: number; height: number; measured: boolean }>({
    width: 0,
    height: 0,
    measured: false,
  })
  useLayoutEffect(() => {
    const element = ref.current
    if (!element) return
    const update = (width: number, height: number) =>
      setBox((current) =>
        current.measured &&
        Math.round(current.width) === Math.round(width) &&
        Math.round(current.height) === Math.round(height)
          ? current
          : { width, height, measured: true },
      )
    const rect = element.getBoundingClientRect()
    update(rect.width, rect.height)
    if (typeof ResizeObserver === 'undefined') return
    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1]
      if (entry) update(entry.contentRect.width, entry.contentRect.height)
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [ref])
  return box
}
