// Adapted from FrameBaker packages/shared/src/types.ts, MIT license.
// Upstream: taotao7/FrameBaker@f9846c985156f83f750bcc654b56cae2a1960229.
// Pisper preserves the uploaded artwork's style instead of forcing pixel art.
export const ACTION_SHEET_MAX_FRAMES = 16

/** @param {number} frameCount */
export function suggestActionSheetGrid(frameCount) {
  const n = Math.max(1, Math.min(ACTION_SHEET_MAX_FRAMES, Math.floor(frameCount) || 1))
  if (n <= 1) return { cols: 1, rows: 1 }
  if (n === 2) return { cols: 2, rows: 1 }
  if (n === 3) return { cols: 3, rows: 1 }
  if (n === 4) return { cols: 4, rows: 1 }
  if (n <= 6) return { cols: 3, rows: 2 }
  return { cols: 4, rows: Math.ceil(n / 4) }
}

/** @param {string} value @param {number} maximum */
function clip(value, maximum) {
  return value.length <= maximum ? value : `${value.slice(0, maximum - 1)}…`
}

/** @param {import('./action-prompts.mjs').ActionSheetPromptOptions} opts */
export function buildActionSheetPrompt(opts) {
  const cols = Math.max(1, Math.min(8, Math.floor(opts.cols) || 1))
  const rows = Math.max(1, Math.min(8, Math.floor(opts.rows) || 1))
  const frames = opts.frames.slice(0, cols * rows)
  const n = frames.length
  const sameAction = n > 0 && frames.every((frame) => frame.id === frames[0].id)
  const first = frames[0]
  const head =
    sameAction && first
      ? `Same character and art style as reference. One ${rows}×${cols} sprite sheet: ${n}-frame continuous ${first.label} cycle, L→R then T→B. Identical look each panel; smooth motion; last loops to first. Plain/transparent bg, no text.`
      : `Same character and art style as reference. One ${rows}×${cols} sprite sheet: ${n}-frame continuous sequence, L→R then T→B. Identical look; smooth panel-to-panel motion. Plain/transparent bg, no text.`
  const parts = [head]
  const character = opts.characterPrompt?.trim()
  if (character) parts.push(`Char: ${clip(character, 160)}`)
  if (n > 0)
    parts.push(
      `Frames: ${frames.map((frame, index) => `${index + 1}:${frame.id}/${frame.prompt}`).join('; ')}`,
    )
  const empty = cols * rows - n
  if (empty > 0) parts.push(`Blank last ${empty} panel(s).`)
  const extra = opts.extra?.trim()
  if (extra) parts.push(clip(extra, 600))
  return clip(parts.join(' '), 1400)
}

/** @param {import('./action-prompts.mjs').CharacterPromptOptions} opts */
export function buildCharacterDirectionSheetPrompt(opts) {
  const parts = [
    'Same character and art style as reference. Create exactly one 8-direction character turnaround sprite sheet arranged as 3 columns × 3 rows with 9 equal cells. MANDATORY: every occupied cell must show a visibly different full-body orientation; use all eight distinct 45-degree body headings exactly once, with no repeated or duplicated view. Cell order is fixed: top-left BACK-LEFT (rear and left side visible); top-center BACK (back faces viewer); top-right BACK-RIGHT (rear and right side visible); middle-left LEFT (left profile); center EMPTY; middle-right RIGHT (right profile); bottom-left FRONT-LEFT (face/chest and left side visible); bottom-center FRONT (face/chest toward viewer); bottom-right FRONT-RIGHT (face/chest and right side visible). Rotate the entire character around the vertical axis—not only the head or eyes—while keeping an identical neutral standing pose. Preserve identity, outfit, equipment, colors, proportions, scale, eye level, orthographic camera and lighting. One full character centered per occupied cell, no overlap; center cell completely empty; plain/transparent background; no text, labels, borders or watermark. Do not fill all cells with the reference orientation.',
  ]
  const character = opts.characterPrompt?.trim()
  if (character)
    parts.push(
      `Appearance only (ignore pose, view and composition in this description): ${clip(character, 180)}`,
    )
  const extra = opts.extra?.trim()
  if (extra) parts.push(clip(extra, 120))
  return clip(parts.join(' '), 1400)
}

/** @param {import('./action-prompts.mjs').ActionVideoPromptOptions} opts */
export function buildActionVideoPrompt(opts) {
  const first = opts.actions[0]
  if (!first)
    return 'Same character and art style as reference. Game character idle loop. Plain bg, no text.'
  const parts = [
    `Same character and art style as reference, performing continuous ${first.prompt} loop. Show the entire character, every limb, accessory, and extremity fully inside the frame at all times; use a slightly wide locked camera and keep about 15% empty safe margin on every edge. Keep the complete action trajectory inside this safe area; never touch or cross the frame boundary and never crop any body part, including at the widest pose. Keep identity consistent; smooth motion; clear silhouette; plain or simple bg; no text, no UI, no watermark.`,
  ]
  const character = opts.characterPrompt?.trim()
  if (character) parts.push(`Char: ${clip(character, 200)}`)
  const extra = opts.extra?.trim()
  if (extra) parts.push(clip(extra, 600))
  return clip(parts.join(' '), 1400)
}
