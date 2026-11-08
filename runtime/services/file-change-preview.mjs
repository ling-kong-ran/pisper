// 文件变更预览：为 edit/write 等文件修改工具生成 diff 预览（统一补丁），
// 供权限审批界面展示；支持 edit 的多次替换与 write 全文替换两种输入。
import { createHash } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { relative } from 'node:path'
import { pisperError } from '../../shared/errors.mjs'
import {
  applyEditsToNormalizedContent,
  generateUnifiedPatch,
  normalizeToLF,
  resolveToCwd,
  stripBom,
} from '../runtime/pi-coding-agent.mjs'

export const MAX_FILE_CHANGE_PREVIEW_BYTES = 2 * 1024 * 1024
export const MAX_FILE_CHANGE_DIFF_CHARS = 200_000

function hashText(value) {
  return createHash('sha256').update(value).digest('hex')
}

function editInput(args) {
  const path = String(args?.path || args?.file_path || '').trim()
  let edits = args?.edits
  if (typeof edits === 'string') {
    try {
      edits = JSON.parse(edits)
    } catch {}
  }
  if (
    !Array.isArray(edits) &&
    typeof args?.oldText === 'string' &&
    typeof args?.newText === 'string'
  )
    edits = [{ oldText: args.oldText, newText: args.newText }]
  if (!path || !Array.isArray(edits) || !edits.length)
    throw pisperError('invalid', 'input', 'edit needs path and edits')
  return { path, edits }
}

function writeInput(args) {
  const path = String(args?.path || args?.file_path || '').trim()
  if (!path || typeof args?.content !== 'string')
    throw pisperError('invalid', 'input', 'write needs path and content')
  return { path, content: args.content }
}

function gitDiffPath(path) {
  const escaped = String(path || '')
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
  return /\s|"|\\/.test(escaped) ? `"${escaped}"` : escaped
}

// 工作区之外的文件（用户批准后可以改）显示为 ../ 开头的相对路径，不展开成含个人目录的绝对路径；
// Windows 跨盘时 relative 只能给出绝对路径，按原样显示。
function fileDiffPath(cwd, absolutePath) {
  const path = relative(cwd, absolutePath).replace(/\\/g, '/')
  if (!path || /[\r\n\0]/.test(path)) throw pisperError('invalid', 'path')
  return path
}

function unifiedFileDiff({ path, before, after, isNew }) {
  const oldPath = isNew ? '/dev/null' : gitDiffPath(`a/${path}`)
  const newPath = gitDiffPath(`b/${path}`)
  const patch = generateUnifiedPatch(path, before, after)
    .replace(/^--- .*$/m, `--- ${oldPath}`)
    .replace(/^\+\+\+ .*$/m, `+++ ${newPath}`)
  return `diff --git ${gitDiffPath(`a/${path}`)} ${newPath}\n${isNew ? 'new file mode 100644\n' : ''}${patch}`
}

function truncateDiff(diff) {
  if (diff.length <= MAX_FILE_CHANGE_DIFF_CHARS) return { diff, truncated: false }
  return { diff: diff.slice(0, MAX_FILE_CHANGE_DIFF_CHARS), truncated: true }
}

async function sourceFile(absolutePath) {
  try {
    const content = await readFile(absolutePath, 'utf8')
    return { exists: true, content }
  } catch (error) {
    if (error?.code === 'ENOENT') return { exists: false, content: '' }
    throw error
  }
}

function validatePreviewSize(before, after) {
  if (Buffer.byteLength(before) > MAX_FILE_CHANGE_PREVIEW_BYTES)
    throw pisperError('too_large', 'file', `max ${MAX_FILE_CHANGE_PREVIEW_BYTES / 1024 / 1024} MB`)
  if (Buffer.byteLength(after) > MAX_FILE_CHANGE_PREVIEW_BYTES)
    throw pisperError(
      'too_large',
      'input',
      `new content max ${MAX_FILE_CHANGE_PREVIEW_BYTES / 1024 / 1024} MB`,
    )
}

/**
 * Builds the exact text mutation preview without writing to the workspace.
 * The returned revision fingerprint lets the caller reject an approval when
 * a separate writer changed the file while the user was reviewing it.
 */
export async function createFileChangePreview({ cwd, toolName, args }) {
  const isEdit = toolName === 'edit'
  const input = isEdit ? editInput(args) : toolName === 'write' ? writeInput(args) : null
  if (!input) return null

  const absolutePath = resolveToCwd(input.path, cwd)
  const source = await sourceFile(absolutePath)
  const path = fileDiffPath(cwd, absolutePath)
  let before
  let after

  if (isEdit) {
    if (!source.exists) throw pisperError('not_found', 'file', path)
    const { text } = stripBom(source.content)
    before = normalizeToLF(text)
    const result = applyEditsToNormalizedContent(before, input.edits, input.path)
    after = result.newContent
    before = result.baseContent
  } else {
    before = normalizeToLF(source.content)
    after = normalizeToLF(input.content)
  }

  validatePreviewSize(before, after)
  const result = truncateDiff(unifiedFileDiff({ path, before, after, isNew: !source.exists }))
  return {
    path,
    diff: result.diff,
    truncated: result.truncated,
    sourceHash: hashText(source.content),
    sourceExists: source.exists,
  }
}

export function sameFileChangeSource(left, right) {
  return Boolean(
    left &&
    right &&
    left.path === right.path &&
    left.sourceExists === right.sourceExists &&
    left.sourceHash === right.sourceHash,
  )
}
