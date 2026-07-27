export const RELEASES_URL = 'https://github.com/ling-kong-ran/pisper/releases'
export const LATEST_RELEASE_API = 'https://api.github.com/repos/ling-kong-ran/pisper/releases/latest'
export const REPOSITORY_URL = 'https://github.com/ling-kong-ran/pisper'
export const REPOSITORY_API = 'https://api.github.com/repos/ling-kong-ran/pisper'
export const DEFAULT_BRANCH = 'main'

export function normalizedVersion(value) {
  return String(value || '').trim().replace(/^v/i, '').split('-')[0]
}

export function newerVersion(candidate, current) {
  const left = normalizedVersion(candidate).split('.').map((value) => Number.parseInt(value, 10) || 0)
  const right = normalizedVersion(current).split('.').map((value) => Number.parseInt(value, 10) || 0)
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    if ((left[index] || 0) !== (right[index] || 0)) return (left[index] || 0) > (right[index] || 0)
  }
  return false
}

/** Prefer the higher of GitHub API and electron-updater channel metadata. */
export function preferredUpdateVersion(githubVersion, updaterVersion) {
  const github = normalizedVersion(githubVersion)
  const updater = normalizedVersion(updaterVersion)
  if (github && updater) return newerVersion(github, updater) ? github : updater
  return github || updater || ''
}

/**
 * Reconcile desktop update UI state.
 * GitHub Releases API is the source of truth for “what is latest”;
 * electron-updater is only trusted for in-app download when its version matches.
 */
export function reconcileDesktopUpdateCheck({
  appVersion,
  githubVersion,
  githubReleaseDate = null,
  githubNotes = '',
  githubReleaseUrl = RELEASES_URL,
  updaterVersion = '',
  updaterIsAvailable = false,
  previousState = 'idle',
  previousAvailableVersion = '',
} = {}) {
  const current = normalizedVersion(appVersion)
  const effectiveVersion = preferredUpdateVersion(githubVersion, updaterVersion) || current
  const available = newerVersion(effectiveVersion, current)
  const metadataSynced = !normalizedVersion(githubVersion)
    || !normalizedVersion(updaterVersion)
    || normalizedVersion(githubVersion) === normalizedVersion(updaterVersion)
  const previousWasDownloaded = previousState === 'downloaded'
    && previousAvailableVersion
    && !newerVersion(effectiveVersion, previousAvailableVersion)
    && !newerVersion(previousAvailableVersion, effectiveVersion)

  if (!available) {
    return {
      state: 'current',
      availableVersion: effectiveVersion,
      releaseDate: githubReleaseDate,
      notes: githubNotes,
      releaseUrl: githubReleaseUrl || RELEASES_URL,
      canDownload: false,
      canInstall: false,
      canResume: false,
      message: '当前已是最新版本。',
    }
  }

  if (previousWasDownloaded && metadataSynced) {
    return {
      state: 'downloaded',
      availableVersion: effectiveVersion,
      releaseDate: githubReleaseDate,
      notes: githubNotes,
      releaseUrl: githubReleaseUrl || RELEASES_URL,
      canDownload: false,
      canInstall: true,
      canResume: false,
      message: '更新已下载，重启后完成安装。',
    }
  }

  return {
    state: 'available',
    availableVersion: effectiveVersion,
    releaseDate: githubReleaseDate,
    notes: githubNotes,
    releaseUrl: githubReleaseUrl || RELEASES_URL,
    canDownload: Boolean(updaterIsAvailable && metadataSynced),
    canInstall: false,
    canResume: false,
    message: metadataSynced
      ? ''
      : '已发现更新，但安装通道元数据尚未同步。请稍后重试检查，或从 GitHub Releases 下载。',
  }
}
