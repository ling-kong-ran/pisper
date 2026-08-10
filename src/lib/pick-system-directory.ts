/**
 * Pick a directory using the most capable native picker available:
 *
 * - desktop shell (Tauri): the system folder dialog via `window.pisperDesktop`
 * - pure Web (Chromium): the browser's native directory picker
 *   (`showDirectoryPicker`); the File System Access handle exposes its parent
 *   chain, so the absolute path can be reconstructed
 * - other browsers: `<input webkitdirectory>` still opens the system folder
 *   picker, but browsers never expose absolute paths there, so a typed-path
 *   fallback is expected by the caller
 */
export async function pickSystemDirectory(initialDirectory = ''): Promise<string | null> {
  const pickDirectory = window.pisperDesktop?.pickDirectory
  if (pickDirectory) return pickDirectory(initialDirectory || undefined)
  return pickWebDirectory()
}

async function pickWebDirectory(): Promise<string | null> {
  if (typeof window === 'undefined') return null
  const showPicker = (window as unknown as { showDirectoryPicker?: () => Promise<unknown> })
    .showDirectoryPicker
  if (showPicker) {
    try {
      const handle = await showPicker()
      return await absolutePathFromHandle(handle)
    } catch {
      return null // cancelled or denied
    }
  }
  return pickWebDirectoryViaFileInput()
}

async function absolutePathFromHandle(handle: unknown): Promise<string | null> {
  const node = handle as {
    name?: string
    parent?: (() => Promise<unknown> | unknown) | unknown
  }
  if (!node || typeof node.name !== 'string' || !node.name) return null
  const parts: string[] = [node.name]
  let current: unknown = node
  for (let guard = 0; guard < 64; guard += 1) {
    const parentRef = (current as { parent?: unknown }).parent
    let parent: unknown = null
    try {
      // Chromium exposes `parent` on directory handles; it may be a plain
      // value, a Promise, or a zero-arg function across versions.
      parent = await (typeof parentRef === 'function' ? parentRef() : parentRef)
    } catch {
      break
    }
    if (!parent) break
    const parentName = (parent as { name?: string } | null)?.name
    if (typeof parentName !== 'string' || !parentName) break
    parts.unshift(parentName)
    current = parent
  }
  return parts.join('/')
}

function pickWebDirectoryViaFileInput(): Promise<string | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input')
    input.type = 'file'
    input.setAttribute('webkitdirectory', '')
    input.setAttribute('directory', '')
    input.style.display = 'none'
    input.addEventListener(
      'change',
      () => {
        const first = input.files?.[0]
        const relative = first?.webkitRelativePath
        // Browsers only expose the path relative to the chosen folder; its
        // first segment is the folder name. Return that so the caller can
        // resolve it relative to the launch workspace.
        resolve(relative ? relative.split('/')[0] : null)
      },
      { once: true },
    )
    input.addEventListener('cancel', () => resolve(null), { once: true })
    document.body.appendChild(input)
    input.click()
    input.remove()
  })
}
