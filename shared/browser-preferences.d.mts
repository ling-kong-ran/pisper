export const BROWSER_PREFERENCE_KEYS: readonly string[]
export const BROWSER_PREFERENCE_MAX_TOTAL_BYTES: number
export class BrowserPreferenceError extends Error {
  code: string
  statusCode: number
}
export function parseBrowserPreferenceUpdates(value: unknown): Record<string, string | null>
export function parseBrowserPreferenceRevisions(
  value: unknown,
  updates: Record<string, string | null>,
  complete?: boolean,
): Record<string, number>
export function parseBrowserPreferenceSnapshot(value: unknown): {
  version: 1
  values: Record<string, string | null>
  revisions: Record<string, number>
}
