import { open } from 'node:fs/promises'
import { extname } from 'node:path'

// 可按文本提取的附件扩展名集合（区别于需 officeparser 解析的文档类型）。
export const ASSET_TEXT_EXTENSIONS = new Set([
  '.txt',
  '.md',
  '.markdown',
  '.json',
  '.jsonl',
  '.js',
  '.jsx',
  '.ts',
  '.tsx',
  '.css',
  '.html',
  '.xml',
  '.yaml',
  '.yml',
  '.csv',
  '.log',
  '.py',
  '.java',
  '.go',
  '.rs',
  '.sh',
  '.ps1',
  '.toml',
  '.sql',
  '.c',
  '.cc',
  '.cpp',
  '.cs',
  '.dart',
  '.env',
  '.graphql',
  '.h',
  '.hpp',
  '.ini',
  '.ipynb',
  '.kt',
  '.kts',
  '.less',
  '.lua',
  '.m',
  '.mm',
  '.php',
  '.properties',
  '.rb',
  '.rst',
  '.sass',
  '.scss',
  '.swift',
  '.tex',
  '.vue',
])

// 需要 officeparser 解析的办公文档扩展名集合。
export const ASSET_DOCUMENT_EXTENSIONS = new Set([
  '.pdf',
  '.docx',
  '.pptx',
  '.xlsx',
  '.odt',
  '.odp',
  '.ods',
  '.rtf',
  '.epub',
])

export const IMAGE_EXTENSIONS = new Set(['.png', '.jpg', '.jpeg', '.gif', '.webp', '.bmp', '.svg'])

// 附件名清洗：去掉换行与尖括号等可能破坏 Markdown/文件名的字符，并限制长度。
export function safeAttachmentName(name) {
  return String(name || '附件')
    .replace(/[\r\n<>]/g, '_')
    .slice(0, 180)
}

// 依据扩展名推断 MIME 类型；未知类型回退到二进制流。
export function mimeFromName(name) {
  const extension = extname(String(name || '')).toLowerCase()
  return (
    {
      '.png': 'image/png',
      '.jpg': 'image/jpeg',
      '.jpeg': 'image/jpeg',
      '.gif': 'image/gif',
      '.webp': 'image/webp',
      '.bmp': 'image/bmp',
      '.svg': 'image/svg+xml',
      '.txt': 'text/plain',
      '.md': 'text/markdown',
      '.markdown': 'text/markdown',
      '.csv': 'text/csv',
      '.log': 'text/plain',
      '.json': 'application/json',
      '.jsonl': 'application/x-ndjson',
      '.js': 'text/javascript',
      '.jsx': 'text/javascript',
      '.ts': 'text/typescript',
      '.tsx': 'text/typescript',
      '.css': 'text/css',
      '.html': 'text/html',
      '.xml': 'application/xml',
      '.yaml': 'application/yaml',
      '.yml': 'application/yaml',
      '.toml': 'application/toml',
      '.py': 'text/x-python',
      '.java': 'text/x-java',
      '.go': 'text/x-go',
      '.rs': 'text/x-rust',
      '.sh': 'text/x-shellscript',
      '.ps1': 'text/plain',
      '.sql': 'application/sql',
      '.c': 'text/x-c',
      '.cc': 'text/x-c++',
      '.cpp': 'text/x-c++',
      '.cs': 'text/plain',
      '.dart': 'text/plain',
      '.env': 'text/plain',
      '.graphql': 'application/graphql',
      '.h': 'text/x-c',
      '.hpp': 'text/x-c++',
      '.ini': 'text/plain',
      '.ipynb': 'application/json',
      '.kt': 'text/plain',
      '.kts': 'text/plain',
      '.less': 'text/css',
      '.lua': 'text/x-lua',
      '.m': 'text/x-objective-c',
      '.mm': 'text/x-objective-c++',
      '.php': 'text/x-php',
      '.properties': 'text/plain',
      '.rb': 'text/x-ruby',
      '.rst': 'text/plain',
      '.sass': 'text/x-sass',
      '.scss': 'text/x-scss',
      '.swift': 'text/x-swift',
      '.tex': 'application/x-tex',
      '.vue': 'text/html',
      '.pdf': 'application/pdf',
      '.docx': 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      '.pptx': 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
      '.xlsx': 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
      '.mp4': 'video/mp4',
      '.webm': 'video/webm',
      '.mov': 'video/quicktime',
      '.m4v': 'video/x-m4v',
      '.avi': 'video/x-msvideo',
      '.mkv': 'video/x-matroska',
      '.mpeg': 'video/mpeg',
      '.mpg': 'video/mpeg',
      '.ogv': 'video/ogg',
    }[extension] || 'application/octet-stream'
  )
}

export function mimeMayContainText(mimeType) {
  const mime = String(mimeType || '')
    .split(';', 1)[0]
    .trim()
    .toLowerCase()
  return (
    mime.startsWith('text/') ||
    [
      'application/json',
      'application/ld+json',
      'application/graphql',
      'application/sql',
      'application/toml',
      'application/xml',
      'application/yaml',
      'application/x-ndjson',
    ].includes(mime) ||
    mime.endsWith('+json') ||
    mime.endsWith('+xml')
  )
}

export function decodeUtf8Text(buffer) {
  try {
    const text = new TextDecoder('utf-8', { fatal: true }).decode(buffer)
    if (text.includes('\0')) return null
    let controls = 0
    for (const character of text) {
      const code = character.codePointAt(0)
      if (code < 32 && !['\n', '\r', '\t', '\f', '\b'].includes(character)) controls += 1
    }
    return controls > Math.max(2, text.length * 0.01) ? null : text
  } catch {
    return null
  }
}

export async function readFilePrefix(path, limit) {
  const handle = await open(path, 'r')
  try {
    const buffer = Buffer.alloc(limit)
    const { bytesRead } = await handle.read(buffer, 0, limit, 0)
    return buffer.subarray(0, bytesRead)
  } finally {
    await handle.close()
  }
}
