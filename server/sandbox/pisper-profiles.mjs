export const DEFAULT_PISPER_SANDBOX_PROFILE = 'system-minimal'

export const PISPER_SANDBOX_LIMITS = Object.freeze({
  wallTimeMs: 10 * 60 * 1000,
  cpuTimeMs: 8 * 60 * 1000,
  memoryBytes: 2 * 1024 * 1024 * 1024,
  processes: 128,
  outputBytes: 50 * 1024 * 1024,
})

export const PISPER_SANDBOX_INHERITED_ENVIRONMENT = Object.freeze(['PATH', 'LANG', 'LC_ALL'])
export const PISPER_SANDBOX_SETTABLE_ENVIRONMENT = Object.freeze([
  'CI',
  'COLORTERM',
  'FORCE_COLOR',
  'NO_COLOR',
  'TERM',
])
