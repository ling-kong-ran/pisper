export async function pickSystemDirectory(initialDirectory = '') {
  const pickDirectory = window.pisperDesktop?.pickDirectory
  if (!pickDirectory) return null
  return pickDirectory(initialDirectory || undefined)
}
