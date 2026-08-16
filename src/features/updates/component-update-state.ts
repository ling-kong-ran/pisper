import type { ComponentUpdateStatus, UpdateStatus } from '@/types/update'

const PROGRESS_STATES = new Set(['available', 'downloading', 'installed', 'error'])

export function currentDesktopVersion(
  hostVersion: string,
  items?: ComponentUpdateStatus[],
): string {
  const current = items?.find((item) => item.component === 'desktop')?.currentVersion.trim()
  return current || hostVersion
}

function progressItems(items: ComponentUpdateStatus[]) {
  return items.filter(
    (item) => item.size > 0 && (PROGRESS_STATES.has(item.state) || item.canInstall),
  )
}

function releaseNotesBody(value: string) {
  return value
    .replace(/\r/g, '')
    .split('\n')
    .filter(
      (line) =>
        !/^\s*#{1,6}\s+Pisper\s+(?:Desktop|TUI|Runtime)\s+v?\d+\.\d+\.\d+\s*$/i.test(line) &&
        !/^\s*#{1,6}\s+What['’]?s Changed\s*$/i.test(line) &&
        !/^\s*(?:\*\*)?(?:Full Changelog|完整变更)(?:\*\*)?\s*[:：].*$/i.test(line),
    )
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

function componentReleaseNotes(items: ComponentUpdateStatus[]) {
  const bodies = items
    .filter((item) => item.state === 'available' || item.state === 'downloading')
    .map((item) => releaseNotesBody(item.notes))
    .filter((value, index, values) => value && values.indexOf(value) === index)
  return bodies.length ? `## What's Changed\n\n${bodies.join('\n\n')}` : ''
}

export function componentUpdateStatus(
  items: ComponentUpdateStatus[],
  checkedAt = new Date().toISOString(),
): UpdateStatus {
  const failed = items.filter((item) => item.state === 'error')
  const available = items.filter((item) => item.state === 'available')
  const checking = items.some((item) => item.state === 'checking')
  const downloading = items.filter((item) => item.state === 'downloading')
  const installed = items.some((item) => item.state === 'installed')
  const release = downloading[0] || available[0] || failed[0] || items[0]
  const notes = componentReleaseNotes(items)
  const progress = progressItems(items)
  const total = progress.reduce((sum, item) => sum + item.size, 0)
  const transferred = progress.reduce((sum, item) => {
    if (item.state === 'installed') return sum + item.size
    if (item.state !== 'downloading') return sum
    return sum + Math.min(item.size, Math.max(0, item.transferred || 0))
  }, 0)

  return {
    state: downloading.length
      ? 'downloading'
      : checking
        ? 'checking'
        : failed.length
          ? 'error'
          : available.length
            ? 'available'
            : installed
              ? 'installed'
              : items.length
                ? 'current'
                : 'idle',
    checkedAt,
    message: failed.map((item) => `${item.component}: ${item.message}`).join('\n'),
    releaseUrl: release?.releaseUrl,
    canDownload: available.length > 0,
    availableVersion: release?.availableVersion,
    notes,
    total,
    transferred,
    percent: total > 0 ? Math.min(100, (transferred / total) * 100) : 0,
  }
}
