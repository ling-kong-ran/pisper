// 状态字形的种类与色调映射：纯数据，供 StatusGlyph、StatusTag 与调用方共用，
// 单独成模块以免组件文件导出常量破坏 Fast Refresh。
// 规格 v2 第 6.5 节：需要你（批准 / 提问 / 待处理的失败）统一是琥珀实心点，原因由旁边的文字说明；
// 其余状态都是中性灰阶，只靠形状区分，所以字形种类按“看起来是什么”而不是按业务状态划分。
export const STATUS_GLYPH_KINDS = [
  'attention',
  'run',
  'queued',
  'done',
  'failed',
  'stopped',
] as const

export type StatusGlyphKind = (typeof STATUS_GLYPH_KINDS)[number]

// 色调对应 --st-* 状态族：只有 attn（需要你）是琥珀，其余状态 token 在 v2 都指向中性文字色。
export type StatusTone = 'run' | 'attn' | 'fail' | 'done' | 'queue'

// failed 指“已处理的失败”（失败 · 已查看）；仍在等人处理的失败属于需要你，用 attention。
export const STATUS_GLYPH_TONES: Readonly<Record<StatusGlyphKind, StatusTone>> = {
  attention: 'attn',
  run: 'run',
  queued: 'queue',
  done: 'done',
  failed: 'fail',
  stopped: 'queue',
}

// 每种状态都必须有文字（规格 6.5：形状 + 文字，不只靠颜色）。基础组件不内置文案，
// 调用方用这里的 i18n 键取默认说明，再按需替换成更具体的原因（如“等你批准”）。
export const STATUS_GLYPH_LABEL_KEYS: Readonly<Record<StatusGlyphKind, string>> = {
  attention: 'common:statusGlyph.attention',
  run: 'common:statusGlyph.run',
  queued: 'common:statusGlyph.queued',
  done: 'common:statusGlyph.done',
  failed: 'common:statusGlyph.failed',
  stopped: 'common:statusGlyph.stopped',
}
