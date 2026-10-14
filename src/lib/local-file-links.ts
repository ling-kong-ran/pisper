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

function hasControlCharacters(value: string) {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0) || 0
    return codePoint < 32 || codePoint === 127
  })
}

function isAbsolutePath(value: string) {
  return /^(?:[A-Za-z]:[\\/]|\/|\\\\)/.test(value)
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
    const pathname = url.pathname
    if (url.hostname && url.hostname !== 'localhost') return `//${url.hostname}${pathname}`
    return /^\/[A-Za-z]:[\\/]/.test(pathname) ? pathname.slice(1) : pathname
  } catch {
    return null
  }
}

// 相对路径只有能定位到工作区根目录时才允许转成哨兵链接：
// 拒绝带 scheme 的 URL、绝对路径形式和任何 .. 段，防止把链接解析到工作区之外。
function isSafeRelativePath(value: string): boolean {
  if (!value || /^[\\/]/.test(value)) return false
  if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(value)) return false
  return !value.split(/[\\/]/).includes('..')
}

// 浏览器端没有可靠的跨平台 path.join：按 base 自身的分隔符拼接，
// 相对部分开头的分隔符去掉，避免产生重复斜杠。
function resolveAgainstBase(base: string, relativePath: string): string {
  const normalizedBase = base.replace(/[\\/]+$/, '')
  if (!normalizedBase)
    return base.startsWith('/') ? '/' + relativePath.replace(/^(?:\.\/)+/, '') : ''
  // 分隔符跟随基址自身（Windows 基址用反斜杠、POSIX 用正斜杠）；
  // 相对部分保留原分隔符，Windows 上两种写法都有效，POSIX 上不能误转反斜杠。
  const separator = base.includes('\\') ? '\\' : '/'
  const cleaned = relativePath.replace(/^[\\/]+/, '').replace(/^(?:\.[/])+/g, '')
  return normalizedBase + separator + cleaned
}

export function parseLocalFileTarget(value: string, baseCwd?: string): LocalFileTarget | null {
  const trimmed = value.trim()
  if (!trimmed || hasControlCharacters(trimmed)) return null

  const isFileUrl = /^file:/i.test(trimmed)
  // 普通 URL 的查询和锚点交给原渲染器；file: 只取 pathname。
  // 在解码前判断，才能保留文件名中编码的问号和井号。
  if (!isFileUrl && /[?#]/.test(trimmed)) return null
  const path = isFileUrl ? fileUrlPath(trimmed) : trimmed
  if (!path) return null

  // 先拆分源码位置，避免 app.ts:12 被当作 scheme；编码的冒号仍属于文件名。
  const target = splitSourcePosition(path)
  const decoded = decodePath(target.path)
  if (!decoded || hasControlCharacters(decoded)) return null
  const normalized = /^\/[A-Za-z]:[\\/]/.test(decoded) ? decoded.slice(1) : decoded
  if (isAbsolutePath(normalized)) return { ...target, path: normalized }

  // 裸 scheme 加数字仍是 URL（例如 javascript:12）；有扩展名的源码引用
  // 才能消除这一歧义，无扩展名的文件可以显式使用 ./README:12。
  if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(path) && !target.path.includes('.')) return null

  // 相对路径必须有绝对工作区根目录，且不得通过 .. 越出工作区。
  if (!baseCwd) return null
  const base = baseCwd.trim()
  if (!isAbsolutePath(base) || hasControlCharacters(base)) return null
  if (!isSafeRelativePath(decoded)) return null
  const resolved = resolveAgainstBase(base, decoded)
  if (!isAbsolutePath(resolved)) return null

  return { ...target, path: resolved }
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
    // searchParams 已还原哨兵传输编码，path 是原生路径，不能再次解码或拆行号。
    if (!isAbsolutePath(path) || hasControlCharacters(path)) return null
    const line = Number(url.searchParams.get('line'))
    const column = Number(url.searchParams.get('column'))
    const decoded: LocalFileTarget = { path }
    if (Number.isSafeInteger(line) && line > 0) decoded.line = line
    if (Number.isSafeInteger(column) && column > 0) decoded.column = column
    return decoded
  } catch {
    return null
  }
}

// 在进入 HTML 安全过滤前把绝对本地路径改写为受控 HTTPS 哨兵；
// 最终渲染器只会把该哨兵交给窄桌面桥接，不会让 WebView 导航到本地文件。
// baseCwd 为会话工作区根目录：提供后相对路径链接（如 workspace/报告.docx）
// 也能解析为哨兵地址，不再被下游安全过滤器显示成 [blocked]。
export function remarkLocalFileLinks(baseCwd?: string) {
  return (tree: MarkdownNode) => {
    const visit = (node: MarkdownNode) => {
      if (node.type === 'link' && typeof node.url === 'string') {
        const target = parseLocalFileTarget(node.url, baseCwd)
        if (target) node.url = encodeLocalFileHref(target)
      }
      for (const child of node.children || []) visit(child)
    }
    visit(tree)
  }
}
