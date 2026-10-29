// 状态字形的种类与色调映射：纯数据，供 StatusGlyph、StatusTag 与调用方共用，
// 单独成模块以免组件文件导出常量破坏 Fast Refresh。
export const STATUS_GLYPH_KINDS = [
  'run',
  'approval',
  'question',
  'failed',
  'queued',
  'done',
  'stopped',
] as const

export type StatusGlyphKind = (typeof STATUS_GLYPH_KINDS)[number]

// 色调对应 --st-* 状态族：run 青、attn 琥珀（只表示需要你）、fail 红、done 绿、queue 灰。
export type StatusTone = 'run' | 'attn' | 'fail' | 'done' | 'queue'

// 等待批准和等你回答同属「需要你」的琥珀；已停止与排队共用中性灰。
export const STATUS_GLYPH_TONES: Readonly<Record<StatusGlyphKind, StatusTone>> = {
  run: 'run',
  approval: 'attn',
  question: 'attn',
  failed: 'fail',
  queued: 'queue',
  done: 'done',
  stopped: 'queue',
}
