// Derived from FrameBaker f9846c985156f83f750bcc654b56cae2a1960229.
// Copyright (c) 2026 taotao7. MIT; see LICENSE and README.md in this directory.
function transformedFrameBounds(width, height, frame) {
  const scale = Math.abs(frame.scale)
  const cos = Math.abs(Math.cos(frame.rotation))
  const sin = Math.abs(Math.sin(frame.rotation))
  const halfW = (width * scale * cos + height * scale * sin) / 2
  const halfH = (width * scale * sin + height * scale * cos) / 2
  return {
    left: frame.offset_x - halfW,
    right: frame.offset_x + halfW,
    top: frame.offset_y - halfH,
    bottom: frame.offset_y + halfH,
  }
}
function transformedFrameRectBounds(width, height, rect, frame) {
  const cos = Math.cos(frame.rotation)
  const sin = Math.sin(frame.rotation)
  const corners = [
    [rect.x - width / 2, rect.y - height / 2],
    [rect.x + rect.w - width / 2, rect.y - height / 2],
    [rect.x + rect.w - width / 2, rect.y + rect.h - height / 2],
    [rect.x - width / 2, rect.y + rect.h - height / 2],
  ].map(([x, y]) => ({
    x: frame.offset_x + (x * cos - y * sin) * frame.scale,
    y: frame.offset_y + (x * sin + y * cos) * frame.scale,
  }))
  return {
    left: Math.min(...corners.map((point) => point.x)),
    right: Math.max(...corners.map((point) => point.x)),
    top: Math.min(...corners.map((point) => point.y)),
    bottom: Math.max(...corners.map((point) => point.y)),
  }
}
function fitScaleForBounds(bounds, viewportWidth, viewportHeight, margin = 0.9) {
  const halfW = Math.max(Math.abs(bounds.left), Math.abs(bounds.right))
  const halfH = Math.max(Math.abs(bounds.top), Math.abs(bounds.bottom))
  if (halfW === 0 || halfH === 0) return 1
  return Math.min(
    1,
    (viewportWidth * margin) / (2 * halfW),
    (viewportHeight * margin) / (2 * halfH),
  )
}
function normalizeFrameRotation(value) {
  const turn = Math.PI * 2
  const normalized = ((((value + Math.PI) % turn) + turn) % turn) - Math.PI
  return Math.min(Math.PI, Math.max(-Math.PI, Math.round(normalized * 1e6) / 1e6))
}
const MAX_SPRITE_SHEET_DIMENSION = 16384
function spriteSheetLayout(
  cellWidth,
  cellHeight,
  count,
  maxDimension = MAX_SPRITE_SHEET_DIMENSION,
) {
  if (cellWidth > maxDimension || cellHeight > maxDimension) {
    throw new Error(
      '\u5355\u5E27\u5C3A\u5BF8\u8D85\u8FC7\u7CBE\u7075\u56FE\u753B\u5E03\u4E0A\u9650\uFF0C\u8BF7\u6539\u7528 PNG \u5E8F\u5217\u5BFC\u51FA',
    )
  }
  const maxColumns = Math.min(count, Math.floor(maxDimension / cellWidth))
  const maxRows = Math.floor(maxDimension / cellHeight)
  const minColumns = Math.max(1, Math.ceil(count / maxRows))
  if (minColumns > maxColumns) {
    throw new Error(
      '\u5E27\u5C3A\u5BF8\u4E0E\u6570\u91CF\u8D85\u8FC7\u7CBE\u7075\u56FE\u753B\u5E03\u4E0A\u9650\uFF0C\u8BF7\u6539\u7528 PNG \u5E8F\u5217\u5BFC\u51FA',
    )
  }
  let columns = minColumns
  let bestScore = Infinity
  for (let candidate = minColumns; candidate <= maxColumns; candidate++) {
    const candidateRows = Math.ceil(count / candidate)
    const width2 = candidate * cellWidth
    const height2 = candidateRows * cellHeight
    const score = Math.max(width2, height2)
    if (score < bestScore) {
      columns = candidate
      bestScore = score
    }
  }
  const rows = Math.max(1, Math.ceil(count / columns))
  const width = columns * cellWidth
  const height = rows * cellHeight
  if (height > maxDimension) {
    throw new Error(
      '\u5E27\u5C3A\u5BF8\u4E0E\u6570\u91CF\u8D85\u8FC7\u7CBE\u7075\u56FE\u753B\u5E03\u4E0A\u9650\uFF0C\u8BF7\u6539\u7528 PNG \u5E8F\u5217\u5BFC\u51FA',
    )
  }
  return { columns, rows, width, height }
}
export {
  MAX_SPRITE_SHEET_DIMENSION,
  fitScaleForBounds,
  normalizeFrameRotation,
  spriteSheetLayout,
  transformedFrameBounds,
  transformedFrameRectBounds,
}
