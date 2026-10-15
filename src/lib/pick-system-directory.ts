// 桌面桥接目录选择：只有 Tauri 桌面壳提供 pickDirectory 时才可用，
// 纯 Web 只能手动填写 Runtime 路径，浏览器的目录句柄不能用作 Node 工作目录。
/** Return whether the desktop shell can provide an absolute directory path. */
export function hasSystemDirectoryPicker(): boolean {
  return typeof window !== 'undefined' && typeof window.pisperDesktop?.pickDirectory === 'function'
}

/** Pick a directory through the Tauri desktop bridge. Pure Web accepts a manually entered Runtime path instead. */
export async function pickSystemDirectory(initialDirectory = ''): Promise<string | null> {
  const pickDirectory = window.pisperDesktop?.pickDirectory
  if (!pickDirectory) return null
  return pickDirectory(initialDirectory || undefined)
}
