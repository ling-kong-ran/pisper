// 人工帧编辑仅描述可重放的修改；源图片和项目状态仍由调用方持有。
/** @param {string} [code] */
function invalid(code = 'workflow_image_invalid_edits') {
  return Object.assign(new Error(code), { code, statusCode: 400 })
}

/** @param {unknown} value @returns {Record<string, unknown>} */
function record(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalid()
  return /** @type {Record<string, unknown>} */ (value)
}

/** @param {unknown} value @param {number} fallback @param {number} min @param {number} max */
function finite(value, fallback, min, max) {
  if (value === undefined) return fallback
  if (typeof value !== 'number' || !Number.isFinite(value) || value < min || value > max)
    throw invalid()
  return value
}

/** @param {unknown} value @returns {import('./image-frame-edits.mjs').ImageFrameEdits} */
export function normalizeImageFrameEdits(value) {
  const input = record(value)
  if (!Array.isArray(input.frames) || input.frames.length < 1 || input.frames.length > 512)
    throw invalid()
  let pointCount = 0
  return {
    frames: input.frames.map((value) => {
      const frame = record(value)
      if (
        !Number.isSafeInteger(frame.sourceIndex) ||
        typeof frame.sourceIndex !== 'number' ||
        frame.sourceIndex < 0 ||
        frame.sourceIndex > 511
      )
        throw invalid()
      const strokes = frame.eraseStrokes === undefined ? [] : frame.eraseStrokes
      if (!Array.isArray(strokes) || strokes.length > 64) throw invalid()
      const durationMs = finite(frame.durationMs, 125, 16, 10000)
      if (!Number.isSafeInteger(durationMs)) throw invalid()
      return {
        sourceIndex: frame.sourceIndex,
        x: finite(frame.x, 0, -4096, 4096),
        y: finite(frame.y, 0, -4096, 4096),
        rotation: finite(frame.rotation, 0, -360, 360),
        scale: finite(frame.scale, 1, 0.05, 8),
        opacity: finite(frame.opacity, 1, 0, 1),
        durationMs,
        eraseStrokes: strokes.map((value) => {
          const stroke = record(value)
          if (
            !Array.isArray(stroke.points) ||
            stroke.points.length < 1 ||
            stroke.points.length > 512 ||
            (stroke.restore !== undefined && typeof stroke.restore !== 'boolean')
          )
            throw invalid()
          pointCount += stroke.points.length
          if (pointCount > 20_000) throw invalid('workflow_image_too_large')
          const radius = finite(stroke.radius, NaN, 0.001, 0.25)
          if (!Number.isFinite(radius)) throw invalid()
          return {
            radius,
            restore: stroke.restore === true,
            points: stroke.points.map((value) => {
              const point = record(value)
              const x = finite(point.x, NaN, 0, 1),
                y = finite(point.y, NaN, 0, 1)
              if (!Number.isFinite(x) || !Number.isFinite(y)) throw invalid()
              return { x, y }
            }),
          }
        }),
      }
    }),
  }
}
