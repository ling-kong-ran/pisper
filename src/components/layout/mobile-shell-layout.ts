// 手机壳只在窄屏用整页切换；宽且高的 Pad 同时露出三栏，避免横屏仍要来回滑动。
export const MOBILE_SHELL_PANES = ['sessions', 'chat', 'context'] as const

export type MobileShellPane = (typeof MOBILE_SHELL_PANES)[number]
export type MobileShellMode = 'phone' | 'pad'
export type MobileContextTab = 'assets' | 'changes' | 'extensions' | 'files' | 'terminal'

const PAD_MIN_WIDTH = 960
const PAD_MIN_HEIGHT = 640
const SWIPE_THRESHOLD = 48

export function resolveMobileShellMode(width: number, height: number): MobileShellMode {
  if (width >= PAD_MIN_WIDTH && height >= PAD_MIN_HEIGHT) return 'pad'
  return 'phone'
}

export function mobileShellPaneIndex(pane: MobileShellPane) {
  const index = MOBILE_SHELL_PANES.indexOf(pane)
  return index < 0 ? 1 : index
}

export function moveMobileShellPane(pane: MobileShellPane, direction: -1 | 1): MobileShellPane {
  const next = mobileShellPaneIndex(pane) + direction
  return MOBILE_SHELL_PANES[Math.min(MOBILE_SHELL_PANES.length - 1, Math.max(0, next))]
}

// 纵向滚动优先。只有明确的水平位移才切屏，避免对话列表被误判成滑动。
export function swipeDeltaToDirection(dx: number, dy: number): -1 | 1 | 0 {
  if (Math.abs(dx) < SWIPE_THRESHOLD || Math.abs(dx) < Math.abs(dy) * 1.2) return 0
  return dx < 0 ? 1 : -1
}

export function shouldIgnoreMobileSwipe(target: EventTarget | null) {
  if (!(target instanceof Element)) return false
  return Boolean(
    target.closest(
      'input, textarea, select, [contenteditable="true"], [data-swipe-ignore], [role="dialog"], [role="slider"]',
    ),
  )
}

export type WorkspaceListEntry = {
  name: string
  kind: 'directory' | 'file'
}

function entryName(item: unknown) {
  const raw =
    typeof item === 'string'
      ? item
      : item && typeof item === 'object' && typeof (item as { name?: unknown }).name === 'string'
        ? (item as { name: string }).name
        : ''
  if (!raw || raw === '.' || raw === '..' || raw.includes('/') || raw.includes('\\')) return ''
  return raw
}

// 兼容两种目录响应：带 directories/files 的选择器，以及 { entries, root } 的运行时列表。
export function normalizeWorkspaceEntries(value: unknown): WorkspaceListEntry[] {
  if (!value || typeof value !== 'object') return []
  const record = value as Record<string, unknown>
  const entries: WorkspaceListEntry[] = []
  const directories = Array.isArray(record.directories) ? record.directories : null
  const files = Array.isArray(record.files) ? record.files : null
  if (directories || files) {
    for (const item of directories ?? []) {
      const name = entryName(item)
      if (name) entries.push({ name, kind: 'directory' })
    }
    for (const item of files ?? []) {
      const name = entryName(item)
      if (name) entries.push({ name, kind: 'file' })
    }
    return entries
  }
  if (!Array.isArray(record.entries)) return []
  for (const item of record.entries) {
    if (!item || typeof item !== 'object') continue
    const row = item as Record<string, unknown>
    const name = entryName(row.name)
    if (!name) continue
    entries.push({
      name,
      kind: row.type === 'directory' || row.kind === 'directory' ? 'directory' : 'file',
    })
  }
  return entries.sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === 'directory' ? -1 : 1
    return a.name.localeCompare(b.name)
  })
}

export function joinWorkspacePath(parent: string, name: string) {
  if (!name || name === '.' || name === '..' || name.includes('/') || name.includes('\\'))
    return parent
  const base = parent.replace(/[\\/]+$/, '')
  return base ? `${base}/${name}` : name
}

export function parentWorkspacePath(path: string) {
  const trimmed = path.replace(/[\\/]+$/, '')
  const index = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  if (index < 0) return ''
  return trimmed.slice(0, index)
}
