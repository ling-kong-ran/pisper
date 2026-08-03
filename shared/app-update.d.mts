export const RELEASES_URL: string
export const LATEST_RELEASE_API: string
export const REPOSITORY_URL: string
export const REPOSITORY_API: string
export const DEFAULT_BRANCH: string

export function normalizedVersion(value: unknown): string
export function newerVersion(candidate: unknown, current: unknown): boolean
export function preferredUpdateVersion(githubVersion: unknown, updaterVersion: unknown): string

export function reconcileDesktopUpdateCheck(
  options?: Record<string, unknown>,
): Record<string, unknown>
