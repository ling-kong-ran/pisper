// Git 差异渲染：把统一 diff 解析成带行号的单元格（上下文/新增/删除），
// 供 Git 变更面板分片展示。
export type GitDiffTone = 'context' | 'added' | 'deleted' | 'empty'

export type GitDiffCell = {
  lineNumber: number | null
  text: string
  tone: GitDiffTone
}

export type GitDiffRow =
  { kind: 'pair'; old: GitDiffCell; next: GitDiffCell } | { kind: 'meta'; text: string }

export type GitDiffHunk = {
  header: string
  rows: GitDiffRow[]
}

export type GitDiffFile = {
  oldPath: string
  newPath: string
  path: string
  metadata: string[]
  hunks: GitDiffHunk[]
}

type PendingLine = { lineNumber: number; text: string }

// 解析差异中的文件路径：去掉引号转义与 a/ b/ 前缀，/dev/null 视为空串；
// Git 引号路径无法解码时保留原样。
function decodedPath(value: string) {
  let path = String(value || '')
    .trim()
    .split('\t')[0]
  if (path.startsWith('"') && path.endsWith('"')) {
    try {
      path = JSON.parse(path)
    } catch {
      // Keep Git's quoted path when it cannot be decoded.
    }
  }
  if (path === '/dev/null') return ''
  return path.replace(/^[ab]\//, '')
}

// 从 diff --git 头解析新旧路径（兼容引号包裹的含空格路径）。
function pathFromDiffHeader(line: string) {
  const quoted = line.match(/^diff --git "a\/(.+)" "b\/(.+)"$/)
  if (quoted) return decodedPath(`b/${quoted[2]}`)
  const plain = line.match(/^diff --git a\/(.+) b\/(.+)$/)
  return plain ? decodedPath(`b/${plain[2]}`) : ''
}

// 空单元格：行号与文本都为空的占位（无对应行的对比侧）。
function emptyCell(): GitDiffCell {
  return { lineNumber: null, text: '', tone: 'empty' }
}

// 从 hunk 头解析起始行号（@@ -old,count +new,count @@）。
function rangeStart(header: string) {
  const match = header.match(/^@@\s+-(\d+)(?:,\d+)?\s+\+(\d+)(?:,\d+)?\s+@@/)
  return {
    oldLine: Number(match?.[1]) || 0,
    newLine: Number(match?.[2]) || 0,
  }
}

// 解析统一 diff 文本：拆分为文件（diff --git 头）+ hunk（@@ 头）+
// 行对（旧/新行号对齐成 pair），供差异查看器逐行渲染。
export function parseUnifiedDiff(value: unknown): GitDiffFile[] {
  const lines = String(value || '')
    .replace(/\r\n/g, '\n')
    .split('\n')
  const files: GitDiffFile[] = []
  let file: GitDiffFile | null = null
  let hunk: GitDiffHunk | null = null
  let oldLine = 0
  let newLine = 0
  let deleted: PendingLine[] = []
  let added: PendingLine[] = []

  const flushChanges = () => {
    if (!hunk || (!deleted.length && !added.length)) return
    const count = Math.max(deleted.length, added.length)
    for (let index = 0; index < count; index += 1) {
      const before = deleted[index]
      const after = added[index]
      hunk.rows.push({
        kind: 'pair',
        old: before
          ? { lineNumber: before.lineNumber, text: before.text, tone: 'deleted' }
          : emptyCell(),
        next: after
          ? { lineNumber: after.lineNumber, text: after.text, tone: 'added' }
          : emptyCell(),
      })
    }
    deleted = []
    added = []
  }

  const flushHunk = () => {
    flushChanges()
    hunk = null
  }

  const flushFile = () => {
    flushHunk()
    if (!file) return
    file.path = file.newPath || file.oldPath || file.path
    files.push(file)
    file = null
  }

  for (const line of lines) {
    if (line.startsWith('diff --git ')) {
      flushFile()
      const fallbackPath = pathFromDiffHeader(line)
      file = { oldPath: '', newPath: '', path: fallbackPath, metadata: [], hunks: [] }
      continue
    }
    if (!file) continue

    if (line.startsWith('@@ ')) {
      flushHunk()
      const range = rangeStart(line)
      oldLine = range.oldLine
      newLine = range.newLine
      hunk = { header: line, rows: [] }
      file.hunks.push(hunk)
      continue
    }

    if (hunk) {
      if (line.startsWith('-')) {
        deleted.push({ lineNumber: oldLine, text: line.slice(1) })
        oldLine += 1
      } else if (line.startsWith('+')) {
        added.push({ lineNumber: newLine, text: line.slice(1) })
        newLine += 1
      } else {
        flushChanges()
        if (line.startsWith(' ')) {
          const text = line.slice(1)
          hunk.rows.push({
            kind: 'pair',
            old: { lineNumber: oldLine, text, tone: 'context' },
            next: { lineNumber: newLine, text, tone: 'context' },
          })
          oldLine += 1
          newLine += 1
        } else if (line.startsWith('\\ ')) {
          hunk.rows.push({ kind: 'meta', text: line.slice(2) })
        }
      }
      continue
    }

    if (line.startsWith('--- ')) file.oldPath = decodedPath(line.slice(4))
    else if (line.startsWith('+++ ')) file.newPath = decodedPath(line.slice(4))
    else if (line.trim()) file.metadata.push(line)
  }

  flushFile()
  return files
}
