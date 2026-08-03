export const RELEASES_URL = 'https://github.com/ling-kong-ran/pisper/releases'
export const REPOSITORY_URL = 'https://github.com/ling-kong-ran/pisper'
export const REPOSITORY_API = 'https://api.github.com/repos/ling-kong-ran/pisper'
export const DEFAULT_BRANCH = 'main'

/**
 * @param {unknown} value
 * @returns {string}
 */
export function normalizedVersion(value) {
  return String(value || '')
    .trim()
    .replace(/^v/i, '')
    .split('-')[0]
}

/**
 * @param {unknown} candidate
 * @param {unknown} current
 * @returns {boolean}
 */
export function newerVersion(candidate, current) {
  const left = normalizedVersion(candidate)
    .split('.')
    .map((value) => Number.parseInt(value, 10) || 0)
  const right = normalizedVersion(current)
    .split('.')
    .map((value) => Number.parseInt(value, 10) || 0)
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    if ((left[index] || 0) !== (right[index] || 0)) return (left[index] || 0) > (right[index] || 0)
  }
  return false
}
