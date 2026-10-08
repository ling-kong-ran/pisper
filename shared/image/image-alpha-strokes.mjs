/**
 * 用线段胶囊覆盖连续笔划，避免低采样率或快速拖动留下孔洞。
 * 函数保持自包含，供 Worker 和内置 iframe 的静态脚本使用同一份像素算法。
 * RGB 始终保留，恢复笔刷只恢复该次编辑原图的 alpha，不会把原有透明区变为不透明。
 * @param {{width:number,height:number,data:Uint8ClampedArray}} source
 * @param {import('./image-frame-edits.mjs').ImageEraseStroke[]} strokes
 * @param {{remaining:number}} [work]
 */
export function applyImageAlphaStrokes(source, strokes, work = { remaining: 128_000_000 }) {
  const data = new Uint8ClampedArray(source.data)
  const { width, height } = source
  for (const stroke of strokes) {
    const radius = stroke.radius * Math.min(width, height)
    const radiusSquared = radius * radius
    for (let index = 0; index < stroke.points.length; index++) {
      const start = stroke.points[index === 0 ? 0 : index - 1]
      const end = stroke.points[index]
      const ax = start.x * width,
        ay = start.y * height
      const bx = end.x * width,
        by = end.y * height
      const dx = bx - ax,
        dy = by - ay,
        lengthSquared = dx * dx + dy * dy
      const left = Math.max(0, Math.floor(Math.min(ax, bx) - radius))
      const right = Math.min(width - 1, Math.ceil(Math.max(ax, bx) + radius))
      const top = Math.max(0, Math.floor(Math.min(ay, by) - radius))
      const bottom = Math.min(height - 1, Math.ceil(Math.max(ay, by) + radius))
      work.remaining -= (right - left + 1) * (bottom - top + 1)
      if (work.remaining < 0)
        throw Object.assign(new Error('workflow_image_too_large'), {
          code: 'workflow_image_too_large',
          statusCode: 400,
        })
      for (let y = top; y <= bottom; y++)
        for (let x = left; x <= right; x++) {
          const px = x + 0.5,
            py = y + 0.5
          const t = lengthSquared
            ? Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / lengthSquared))
            : 0
          const distance = (px - ax - t * dx) ** 2 + (py - ay - t * dy) ** 2
          if (distance <= radiusSquared) {
            const alpha = (y * width + x) * 4 + 3
            data[alpha] = stroke.restore ? source.data[alpha] : 0
          }
        }
    }
  }
  return data
}
