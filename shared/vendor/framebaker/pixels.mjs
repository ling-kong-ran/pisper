// Derived from FrameBaker f9846c985156f83f750bcc654b56cae2a1960229.
// Copyright (c) 2026 taotao7. MIT; see LICENSE and README.md in this directory.
function removeColorPixels(data, width, height, options) {
  const out = new Uint8ClampedArray(data)
  if (!options.targets.length) return out
  const tolerance = Math.max(0, Math.min(255, Math.round(options.tolerance)))
  const softness = Math.max(0, Math.min(64, Math.round(options.softness ?? 0)))
  const pixels = Math.min(width * height, Math.floor(data.length / 4))
  for (let pixel = 0; pixel < pixels; pixel++) {
    const offset = pixel * 4
    let distance = 255
    for (const target of options.targets) {
      distance = Math.min(
        distance,
        Math.max(
          Math.abs(data[offset] - target[0]),
          Math.abs(data[offset + 1] - target[1]),
          Math.abs(data[offset + 2] - target[2]),
        ),
      )
      if (distance === 0) break
    }
    if (distance <= tolerance) {
      out[offset + 3] = 0
    } else if (softness > 0 && distance < tolerance + softness) {
      out[offset + 3] = Math.round((data[offset + 3] * (distance - tolerance)) / softness)
    }
  }
  return out
}
function extractPalette(data, width, height, maxColors) {
  const limit = Math.max(0, Math.floor(maxColors))
  if (limit === 0) return []
  const pixels = Math.min(width * height, Math.floor(data.length / 4))
  const bucketCounts = new Uint32Array(1 << 15)
  for (let pixel = 0; pixel < pixels; pixel++) {
    const offset = pixel * 4
    if (data[offset + 3] < 128) continue
    const bucket =
      ((data[offset] >> 3) << 10) | ((data[offset + 1] >> 3) << 5) | (data[offset + 2] >> 3)
    bucketCounts[bucket]++
  }
  const buckets = Array.from(bucketCounts, (count, bucket) => ({ bucket, count }))
    .filter((entry) => entry.count > 0)
    .sort((a, b) => b.count - a.count || a.bucket - b.bucket)
    .slice(0, limit)
  if (!buckets.length) return []
  const selected = new Map(buckets.map((entry, index) => [entry.bucket, index]))
  const exactCounts = buckets.map(() => /* @__PURE__ */ new Map())
  for (let pixel = 0; pixel < pixels; pixel++) {
    const offset = pixel * 4
    if (data[offset + 3] < 128) continue
    const red = data[offset]
    const green = data[offset + 1]
    const blue = data[offset + 2]
    const bucket = ((red >> 3) << 10) | ((green >> 3) << 5) | (blue >> 3)
    const index = selected.get(bucket)
    if (index == null) continue
    const color = (red << 16) | (green << 8) | blue
    const counts = exactCounts[index]
    counts.set(color, (counts.get(color) ?? 0) + 1)
  }
  return exactCounts.map((counts) => {
    let bestColor = 0
    let bestCount = -1
    for (const [color, count] of counts) {
      if (count > bestCount) {
        bestColor = color
        bestCount = count
      }
    }
    return [(bestColor >> 16) & 255, (bestColor >> 8) & 255, bestColor & 255]
  })
}
function computeOpaqueBounds(data, width, height, alphaThreshold = 0) {
  let minX = width
  let minY = height
  let maxX = -1
  let maxY = -1
  for (let y = 0; y < height; y++) {
    const row = y * width * 4
    for (let x = 0; x < width; x++) {
      if (data[row + x * 4 + 3] > alphaThreshold) {
        if (x < minX) minX = x
        if (x > maxX) maxX = x
        if (y < minY) minY = y
        if (y > maxY) maxY = y
      }
    }
  }
  if (maxX < 0) return null
  return { x: minX, y: minY, w: maxX - minX + 1, h: maxY - minY + 1 }
}
const ANALYSIS_ALPHA_THRESHOLD = 8
function detectOpaqueComponents(data, width, height, options = {}) {
  const alphaThreshold = options.alphaThreshold ?? ANALYSIS_ALPHA_THRESHOLD
  const total = width * height
  if (total <= 0) return []
  const foreground = (index) => data[index * 4 + 3] > alphaThreshold
  let opaquePixels = 0
  for (let i = 0; i < total; i++) if (foreground(i)) opaquePixels++
  if (opaquePixels === 0) return []
  const visited = new Uint8Array(total)
  const queue = new Int32Array(total)
  const components = []
  for (let start = 0; start < total; start++) {
    if (visited[start] || !foreground(start)) continue
    let read = 0
    let write = 0
    let area = 0
    let minX = width,
      minY = height,
      maxX = -1,
      maxY = -1
    visited[start] = 1
    queue[write++] = start
    while (read < write) {
      const index = queue[read++]
      area++
      const x = index % width
      const y = (index - x) / width
      if (x < minX) minX = x
      if (x > maxX) maxX = x
      if (y < minY) minY = y
      if (y > maxY) maxY = y
      const visit = (next) => {
        if (!visited[next] && foreground(next)) {
          visited[next] = 1
          queue[write++] = next
        }
      }
      if (x > 0) visit(index - 1)
      if (x + 1 < width) visit(index + 1)
      if (y > 0) visit(index - width)
      if (y + 1 < height) visit(index + width)
    }
    components.push({ area, rect: { x: minX, y: minY, w: maxX - minX + 1, h: maxY - minY + 1 } })
  }
  const minArea = Math.max(
    options.minAreaPixels ?? 16,
    Math.ceil(opaquePixels * (options.minAreaRatio ?? 5e-3)),
  )
  let significant = components.filter((component) => component.area >= minArea)
  if (!significant.length)
    significant = [
      components.reduce((largest, current) => (current.area > largest.area ? current : largest)),
    ]
  if (options.maxComponents && significant.length > options.maxComponents) {
    significant = [...significant].sort((a, b) => b.area - a.area).slice(0, options.maxComponents)
  }
  const sortedHeights = significant.map((component) => component.rect.h).sort((a, b) => a - b)
  const medianHeight = sortedHeights.length
    ? sortedHeights[Math.floor(sortedHeights.length / 2)]
    : 1
  const band = Math.max(1, medianHeight * 0.6)
  return significant
    .map((component) => ({
      rect: component.rect,
      cx: component.rect.x + component.rect.w / 2,
      cy: component.rect.y + component.rect.h / 2,
    }))
    .sort((a, b) => Math.floor(a.cy / band) - Math.floor(b.cy / band) || a.cx - b.cx)
    .map((entry) => entry.rect)
}
export { computeOpaqueBounds, detectOpaqueComponents, extractPalette, removeColorPixels }
