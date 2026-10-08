// @public 工作区目录浏览接口：集中封装请求与兼容响应归一化。
import { apiJson } from '@/lib/http/api'
import { invalidResponseError } from '@/lib/http/http-response'

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
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw invalidResponseError()
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
  if (!Array.isArray(record.entries)) throw invalidResponseError()
  for (const item of record.entries) {
    if (!item || typeof item !== 'object') throw invalidResponseError()
    const row = item as Record<string, unknown>
    const name = entryName(row.name)
    if (!name || !['directory', 'file'].includes(String(row.type ?? row.kind)))
      throw invalidResponseError()
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

export async function listWorkspaceEntries(path: string, signal: AbortSignal) {
  const response = await apiJson<unknown>(
    `/api/workspace-entries?path=${encodeURIComponent(path)}`,
    { signal },
  )
  return normalizeWorkspaceEntries(response)
}
