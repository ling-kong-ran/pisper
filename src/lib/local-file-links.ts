const LOCAL_FILE_LINK_ORIGIN = 'https://local-file.pisper.invalid'
const LOCAL_FILE_LINK_PATH = '/reveal'

export type LocalFileTarget = {
  path: string
  line?: number
  column?: number
}

type MarkdownNode = {
  type?: string
  url?: unknown
  children?: MarkdownNode[]
}

function decodePath(value: string) {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

function splitSourcePosition(path: string): LocalFileTarget {
  const match = path.match(/:(\d+)(?::(\d+))?$/)
  if (!match) return { path }
  const target: LocalFileTarget = {
    path: path.slice(0, -match[0].length),
    line: Number(match[1]),
  }
  if (match[2]) target.column = Number(match[2])
  return target
}

function fileUrlPath(value: string) {
  try {
    const url = new URL(value)
    if (url.protocol !== 'file:' || url.username || url.password || url.port) return null
    const pathname = decodePath(url.pathname)
    if (url.hostname && url.hostname !== 'localhost') return `//${url.hostname}${pathname}`
    return /^\/[A-Za-z]:[\\/]/.test(pathname) ? pathname.slice(1) : pathname
  } catch {
    return null
  }
}

export function parseLocalFileTarget(value: string): LocalFileTarget | null {
  const trimmed = value.trim()
  if (
    !trimmed ||
    [...trimmed].some((character) => {
      const codePoint = character.codePointAt(0) || 0
      return codePoint < 32 || codePoint === 127
    })
  )
    return null

  const decoded = /^file:/i.test(trimmed) ? fileUrlPath(trimmed) : decodePath(trimmed)
  if (!decoded) return null
  const normalized = /^\/[A-Za-z]:[\\/]/.test(decoded) ? decoded.slice(1) : decoded
  if (!/^(?:[A-Za-z]:[\\/]|\/|\\\\)/.test(normalized)) return null

  const target = splitSourcePosition(normalized)
  return target.path ? target : null
}

export function encodeLocalFileHref(target: LocalFileTarget) {
  const url = new URL(LOCAL_FILE_LINK_PATH, LOCAL_FILE_LINK_ORIGIN)
  url.searchParams.set('path', target.path)
  if (target.line !== undefined) url.searchParams.set('line', String(target.line))
  if (target.column !== undefined) url.searchParams.set('column', String(target.column))
  return url.href
}

export function decodeLocalFileHref(value: string): LocalFileTarget | null {
  try {
    const url = new URL(value)
    if (url.origin !== LOCAL_FILE_LINK_ORIGIN || url.pathname !== LOCAL_FILE_LINK_PATH) return null
    const path = url.searchParams.get('path') || ''
    const target = parseLocalFileTarget(path)
    if (!target) return null
    const line = Number(url.searchParams.get('line'))
    const column = Number(url.searchParams.get('column'))
    const decoded: LocalFileTarget = { path: target.path }
    if (Number.isSafeInteger(line) && line > 0) decoded.line = line
    if (Number.isSafeInteger(column) && column > 0) decoded.column = column
    return decoded
  } catch {
    return null
  }
}

// 在进入 HTML 安全过滤前把绝对本地路径改写为受控 HTTPS 哨兵；
// 最终渲染器只会把该哨兵交给窄桌面桥接，不会让 WebView 导航到本地文件。
export function remarkLocalFileLinks() {
  return (tree: MarkdownNode) => {
    const visit = (node: MarkdownNode) => {
      if (node.type === 'link' && typeof node.url === 'string') {
        const target = parseLocalFileTarget(node.url)
        if (target) node.url = encodeLocalFileHref(target)
      }
      for (const child of node.children || []) visit(child)
    }
    visit(tree)
  }
}
