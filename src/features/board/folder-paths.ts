// 新建文件夹对话框用到的路径拼写：只在界面上推算“上级目录”和“将新建的路径”，不做校验；
// 真正的路径解析与校验在 Runtime（/api/workspace-folders）。路径可能来自 Windows 主机，
// 分隔符按路径本身判断，不按浏览器所在平台。

// 路径里只出现反斜杠时按 Windows 路径拼写，否则用正斜杠。
export function folderSeparator(path: string) {
  return path.includes('\\') && !path.includes('/') ? '\\' : '/'
}

// 上级目录：/a/b → /a，/a → /，C:\a → C:\；没有上级（空串、相对名称、根目录）时返回空串，
// 交给 Runtime 用默认工作目录。
export function parentDirectory(path: string) {
  const trimmed = path.trim().replace(/[\\/]+$/, '')
  const index = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  if (index < 0) return ''
  const parent = trimmed.slice(0, index)
  if (!parent) return trimmed[0]
  if (/^[a-z]:$/i.test(parent)) return `${parent}${trimmed[index]}`
  return parent
}

export function joinFolderPath(parent: string, name: string) {
  const base = parent.trim()
  if (!base) return name
  return /[\\/]$/.test(base) ? `${base}${name}` : `${base}${folderSeparator(base)}${name}`
}
