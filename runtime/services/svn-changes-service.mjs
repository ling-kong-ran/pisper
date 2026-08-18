// SVN 变更服务：与 GitChangesService 对等的 SVN 实现（status/diff/commit/push/revert），
// 未装 svn 或非 SVN 工作区时静默降级。
import { execFile } from 'node:child_process'
import { readFile, rm, stat } from 'node:fs/promises'
import { resolve, sep } from 'node:path'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const SVN_TIMEOUT_MS = 30_000
const COMMIT_TIMEOUT_MS = 120_000
const MAX_DIFF_CHARS = 200_000
const MAX_UNTRACKED_BYTES = 512 * 1024
const MAX_COMMIT_MESSAGE_CHARS = 4_000

const STATUS_CODES = {
  modified: 'M',
  added: 'A',
  deleted: 'D',
  missing: '!',
  unversioned: '?',
  replaced: 'R',
  conflicted: 'C',
  obstructed: '~',
}

async function runSvn(cwd, args, { timeout = SVN_TIMEOUT_MS } = {}) {
  try {
    const result = await execFileAsync('svn', args, {
      cwd,
      timeout,
      windowsHide: true,
      maxBuffer: 16 * 1024 * 1024,
      encoding: 'utf8',
    })
    return { ok: true, stdout: result.stdout || '', stderr: result.stderr || '' }
  } catch (error) {
    const stderr = String(error?.stderr || '').trim()
    const stdout = String(error?.stdout || '').trim()
    return {
      ok: false,
      stdout,
      stderr,
      message: stderr || stdout || String(error?.message || error),
    }
  }
}

function gitDiffPath(path) {
  const escaped = String(path || '')
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
  return /\s|"|\\/.test(escaped) ? `"${escaped}"` : escaped
}

function decodeXmlEntities(value) {
  return String(value || '')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;|&apos;/g, "'")
    .replace(/&amp;/g, '&')
}

export function parseSvnStatusXml(output) {
  const files = []
  const entryPattern = /<entry\s+path="([^"]*)"[^>]*>([\s\S]*?)<\/entry>/g
  for (const match of String(output || '').matchAll(entryPattern)) {
    const item = /<wc-status\b[^>]*?\bitem="([^"]*)"/.exec(match[2])?.[1] || ''
    const status = STATUS_CODES[item]
    if (!status) continue
    files.push({ path: decodeXmlEntities(match[1]), status })
  }
  return files
}

export function normalizeSvnDiff(output) {
  const lines = String(output || '')
    .split('\n')
    .map((line) => line.replace(/\r$/, ''))
  const normalized = []
  let propertyBlock = false
  for (const line of lines) {
    const indexMatch = /^Index: (.+)$/.exec(line)
    if (indexMatch) {
      propertyBlock = false
      const path = indexMatch[1]
      normalized.push(`diff --git ${gitDiffPath(`a/${path}`)} ${gitDiffPath(`b/${path}`)}`)
      continue
    }
    if (propertyBlock) continue
    if (/^={10,}$/.test(line)) continue
    if (line.startsWith('Property changes on:')) {
      propertyBlock = true
      continue
    }
    const marker = /^(---|\+\+\+) (.*?)\t\((revision \d+|working copy|nonexistent|deleted)\)$/.exec(
      line,
    )
    if (marker) {
      const [, sign, path, kind] = marker
      if (sign === '---') {
        normalized.push(kind === 'revision 0' ? '--- /dev/null' : `--- a/${path}`)
      } else {
        normalized.push(kind === 'working copy' ? `+++ b/${path}` : '+++ /dev/null')
      }
      continue
    }
    normalized.push(line)
  }
  return normalized.join('\n')
}

async function buildAddedFileDiff(cwd, relativePath) {
  try {
    const absolute = resolve(cwd, relativePath)
    const root = resolve(cwd)
    if (absolute !== root && !absolute.startsWith(`${root}${sep}`)) return ''
    const info = await stat(absolute)
    if (!info.isFile() || info.size > MAX_UNTRACKED_BYTES) return ''
    const buffer = await readFile(absolute)
    if (buffer.includes(0)) return ''
    const lines = buffer.toString('utf8').replace(/\r\n/g, '\n').split('\n')
    if (lines.length && lines[lines.length - 1] === '') lines.pop()
    const header = `diff --git ${gitDiffPath(`a/${relativePath}`)} ${gitDiffPath(`b/${relativePath}`)}\n--- /dev/null\n+++ b/${relativePath}\n`
    if (!lines.length) return header
    const body = `@@ -0,0 +1,${lines.length} @@\n${lines.map((line) => `+${line}`).join('\n')}\n`
    return header + body
  } catch {
    return ''
  }
}

function validateCommitMessage(message) {
  const text = String(message || '').trim()
  if (!text) throw new Error('Commit message 不能为空。')
  if (text.length > MAX_COMMIT_MESSAGE_CHARS)
    throw new Error(`Commit message 不能超过 ${MAX_COMMIT_MESSAGE_CHARS} 个字符。`)
  return text
}

export class SvnChangesService {
  async getChanges(cwd) {
    const info = await runSvn(cwd, ['info'])
    if (!info.ok) {
      const svnMissing = /ENOENT|not recognized|command not found/i.test(info.message || '')
      return {
        vcs: '',
        isRepo: false,
        svnAvailable: !svnMissing,
        cwd,
        branch: '',
        hasHead: false,
        files: [],
        diff: '',
        diffTruncated: false,
        ahead: null,
        error: '',
      }
    }
    const statusResult = await runSvn(cwd, ['status', '--xml'])
    const files = statusResult.ok ? parseSvnStatusXml(statusResult.stdout) : []
    let diff = ''
    let diffTruncated = false
    if (files.length) {
      const diffResult = await runSvn(cwd, ['diff'])
      if (diffResult.ok) diff = normalizeSvnDiff(diffResult.stdout)
      for (const file of files.filter((item) => item.status === '?')) {
        if (diff.length >= MAX_DIFF_CHARS) break
        const addedDiff = await buildAddedFileDiff(cwd, file.path)
        if (addedDiff) diff += `${diff && !diff.endsWith('\n') ? '\n' : ''}${addedDiff}`
      }
      if (diff.length > MAX_DIFF_CHARS) {
        diff = diff.slice(0, MAX_DIFF_CHARS)
        diffTruncated = true
      }
    }
    return {
      vcs: 'svn',
      isRepo: true,
      svnAvailable: true,
      cwd,
      branch: '',
      hasHead: true,
      files,
      diff,
      diffTruncated,
      ahead: null,
      error: '',
    }
  }

  async commit(cwd, message) {
    const text = validateCommitMessage(message)
    const changes = await this.getChanges(cwd)
    if (!changes.isRepo) throw new Error('当前目录不是 SVN 工作区。')
    for (const file of changes.files.filter((item) => item.status === '?')) {
      const addResult = await runSvn(cwd, ['add', file.path])
      if (!addResult.ok && !/already under version control/i.test(addResult.message))
        throw new Error(`svn add 失败：${addResult.message}`)
    }
    for (const file of changes.files.filter((item) => item.status === '!')) {
      const deleteResult = await runSvn(cwd, ['delete', '--force', file.path])
      if (!deleteResult.ok) throw new Error(`svn delete 失败：${deleteResult.message}`)
    }
    const commitResult = await runSvn(cwd, ['commit', '-m', text], { timeout: COMMIT_TIMEOUT_MS })
    if (!commitResult.ok) throw new Error(`svn commit 失败：${commitResult.message}`)
    return this.getChanges(cwd)
  }

  async revert(cwd) {
    const changes = await this.getChanges(cwd)
    if (!changes.isRepo) throw new Error('当前目录不是 SVN 工作区。')
    const revertResult = await runSvn(cwd, ['revert', '-R', '.'])
    if (!revertResult.ok) throw new Error(`svn revert 失败：${revertResult.message}`)
    const root = resolve(cwd)
    for (const file of changes.files.filter((item) => item.status === '?')) {
      const absolute = resolve(cwd, file.path)
      if (absolute === root || !absolute.startsWith(`${root}${sep}`)) continue
      await rm(absolute, { recursive: true, force: true })
    }
    return this.getChanges(cwd)
  }
}
