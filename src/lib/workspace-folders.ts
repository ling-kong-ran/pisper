// 列出 Runtime 所在电脑上某个文件夹里的子文件夹：手机、配对设备看不到主机的系统文件管理器，
// 目录选择面板靠它逐级点选。只有文件夹名和路径，不含文件。
import { apiJson } from '@/lib/api'

export type WorkspaceFolder = { name: string; path: string }

export type WorkspaceFolderListing = {
  path: string
  /** 上一级的路径；已经到根时为空。 */
  parent: string
  folders: WorkspaceFolder[]
  truncated: boolean
}

/** path 为空时从 Runtime 的默认工作目录开始。 */
export async function listWorkspaceFolders(
  path: string,
  signal?: AbortSignal,
): Promise<WorkspaceFolderListing> {
  const query = path ? `?path=${encodeURIComponent(path)}` : ''
  return parseWorkspaceFolderListing(
    await apiJson<unknown>(`/api/workspace-folders${query}`, { signal }),
  )
}

export function parseWorkspaceFolderListing(value: unknown): WorkspaceFolderListing {
  const record = value && typeof value === 'object' ? (value as Record<string, unknown>) : {}
  const text = (field: unknown) => (typeof field === 'string' ? field : '')
  const folders = Array.isArray(record.folders) ? record.folders : []
  return {
    path: text(record.path),
    parent: text(record.parent),
    folders: folders.flatMap((folder) => {
      const entry = folder && typeof folder === 'object' ? (folder as Record<string, unknown>) : {}
      const name = text(entry.name)
      const path = text(entry.path)
      return name && path ? [{ name, path }] : []
    }),
    truncated: record.truncated === true,
  }
}
