// 桌面桥接目录选择：只有 Tauri 桌面壳提供 pickDirectory 时才可用，
// 纯 Web 环境走 WorkspacePicker（浏览器无系统级目录选择能力）。
/** Return whether the desktop shell can provide an absolute directory path. */
export function hasSystemDirectoryPicker(): boolean {
  return typeof window !== 'undefined' && typeof window.pisperDesktop?.pickDirectory === 'function'
}

/** Pick a directory through the Tauri desktop bridge. Pure Web uses WorkspacePicker instead. */
export async function pickSystemDirectory(initialDirectory = ''): Promise<string | null> {
  const pickDirectory = window.pisperDesktop?.pickDirectory
  if (!pickDirectory) return null
  return pickDirectory(initialDirectory || undefined)
}
