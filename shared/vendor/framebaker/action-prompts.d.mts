export type ActionPrompt = { id: string; label: string; prompt: string }
export type CharacterPromptOptions = { characterPrompt?: string | null; extra?: string | null }
export type ActionSheetPromptOptions = CharacterPromptOptions & {
  frames: ActionPrompt[]
  cols: number
  rows: number
}
export type ActionVideoPromptOptions = CharacterPromptOptions & { actions: ActionPrompt[] }
export const ACTION_SHEET_MAX_FRAMES: 16
export function suggestActionSheetGrid(frameCount: number): { cols: number; rows: number }
export function buildActionSheetPrompt(opts: ActionSheetPromptOptions): string
export function buildCharacterDirectionSheetPrompt(opts: CharacterPromptOptions): string
export function buildActionVideoPrompt(opts: ActionVideoPromptOptions): string
