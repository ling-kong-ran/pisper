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
