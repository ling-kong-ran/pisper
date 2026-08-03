export const PETDEX_PAGE_URL = 'https://petdex.dev'
export const PET_FRAME_WIDTH = 192
export const PET_FRAME_HEIGHT = 208
export const PET_SHEET_COLUMNS = 8
export const PET_MINIMUM_ROWS = 9
export const PET_WINDOW_WIDTH = 192
export const PET_WINDOW_HEIGHT = 288
export const MAX_PET_BYTES = 16 * 1024 * 1024

export const PET_STATES = Object.freeze({
  idle: { row: 0, frames: 6, durationMs: 1100 },
  'running-right': { row: 1, frames: 8, durationMs: 1060 },
  'running-left': { row: 2, frames: 8, durationMs: 1060 },
  waving: { row: 3, frames: 4, durationMs: 700 },
  jumping: { row: 4, frames: 5, durationMs: 840 },
  failed: { row: 5, frames: 8, durationMs: 1220 },
  waiting: { row: 6, frames: 6, durationMs: 1010 },
  running: { row: 7, frames: 6, durationMs: 820 },
  review: { row: 8, frames: 6, durationMs: 1030 },
})

export function readImageDimensions(buffer) {
  if (!Buffer.isBuffer(buffer)) return null
  if (
    buffer.length >= 24 &&
    buffer.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]))
  ) {
    return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20), mime: 'image/png' }
  }
  if (buffer.length < 30) return null
  if (buffer.toString('ascii', 0, 4) !== 'RIFF' || buffer.toString('ascii', 8, 12) !== 'WEBP')
    return null
  const chunk = buffer.toString('ascii', 12, 16)
  if (chunk === 'VP8X') {
    return {
      width: 1 + buffer.readUIntLE(24, 3),
      height: 1 + buffer.readUIntLE(27, 3),
      mime: 'image/webp',
    }
  }
  if (chunk === 'VP8L' && buffer[20] === 0x2f) {
    return {
      width: 1 + buffer[21] + ((buffer[22] & 0x3f) << 8),
      height: 1 + (buffer[22] >> 6) + (buffer[23] << 2) + ((buffer[24] & 0x0f) << 10),
      mime: 'image/webp',
    }
  }
  if (chunk === 'VP8 ' && buffer[23] === 0x9d && buffer[24] === 0x01 && buffer[25] === 0x2a) {
    return {
      width: buffer.readUInt16LE(26) & 0x3fff,
      height: buffer.readUInt16LE(28) & 0x3fff,
      mime: 'image/webp',
    }
  }
  return null
}

export function isPetSheetDimensions({ width, height } = {}) {
  return (
    width === PET_FRAME_WIDTH * PET_SHEET_COLUMNS &&
    Number.isInteger(height) &&
    height >= PET_FRAME_HEIGHT * PET_MINIMUM_ROWS &&
    height % PET_FRAME_HEIGHT === 0
  )
}

export function normalizePetOpacity(value, fallback = 1) {
  const opacity = Number(value)
  if (!Number.isFinite(opacity)) return fallback
  return Math.round(Math.max(0.2, Math.min(1, opacity)) * 100) / 100
}

export function petStateForAgentEvent(event) {
  if (event === 'error') return 'failed'
  if (event === 'done') return 'waving'
  if (event === 'tool_start' || event === 'tool_update' || event === 'tool_end') return 'running'
  if (event === 'thinking_patch' || event === 'thinking_reset' || event === 'compaction_start')
    return 'review'
  if (event === 'meta' || event === 'text_patch' || event === 'retry' || event === 'queue_update')
    return 'waiting'
  return null
}

export function petBubbleKeyForState(state) {
  if (state === 'review') return 'pet.bubbleThinking'
  if (state === 'running') return 'pet.bubbleWorking'
  if (state === 'waiting') return 'pet.bubbleResponding'
  if (state === 'waving') return 'pet.bubbleDone'
  if (state === 'failed') return 'pet.bubbleFailed'
  if (state === 'jumping') return 'pet.bubbleHello'
  return ''
}

export function resolvePetPosition(saved, displays, primaryDisplay, margin = 20) {
  const validDisplays = Array.isArray(displays) ? displays : []
  const x = Number(saved?.x)
  const y = Number(saved?.y)
  const legacyTopLeftDefault = x === 0 && y === 0 && saved?.customized !== true
  if (Number.isFinite(x) && Number.isFinite(y) && !legacyTopLeftDefault) {
    const visible = validDisplays.some(({ workArea }) => {
      if (!workArea) return false
      return (
        x + PET_WINDOW_WIDTH > workArea.x &&
        x < workArea.x + workArea.width &&
        y + PET_WINDOW_HEIGHT > workArea.y &&
        y < workArea.y + workArea.height
      )
    })
    if (visible) return { x: Math.round(x), y: Math.round(y) }
  }

  const workArea = primaryDisplay?.workArea || { x: 0, y: 0, width: 1280, height: 720 }
  return {
    x: Math.round(workArea.x + workArea.width - PET_WINDOW_WIDTH - margin),
    y: Math.round(workArea.y + workArea.height - PET_WINDOW_HEIGHT - margin),
  }
}
