// Git 变更服务：查询工作区 Git 状态/差异，执行提交/推送/撤销；
// 通过 execFile 调 git 并捕获错误（目录非 Git 仓库时返回 isRepo: false）。
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const GIT_TIMEOUT_MS = 30_000
const PUSH_TIMEOUT_MS = 120_000
const MAX_DIFF_CHARS = 200_000
const MAX_COMMIT_MESSAGE_CHARS = 4_000

async function runGit(cwd, args, { timeout = GIT_TIMEOUT_MS } = {}) {
  try {
    const result = await execFileAsync('git', args, {
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

function parsePorcelainStatus(output) {
  const files = []
  for (const line of String(output || '').split('\n')) {
    if (!line.trim() || line.length < 4) continue
    const status = line.slice(0, 2)
    let path = line.slice(3)
    const renameArrow = path.indexOf(' -> ')
    if (renameArrow !== -1) path = path.slice(renameArrow + 4)
    if (path.startsWith('"') && path.endsWith('"')) {
      try {
        path = JSON.parse(path)
      } catch {}
    }
    files.push({ path, status: status.trim() || status })
  }
  return files
}

export class GitChangesService {
  async getChanges(cwd) {
    const repoCheck = await runGit(cwd, ['rev-parse', '--is-inside-work-tree'])
    if (!repoCheck.ok || repoCheck.stdout.trim() !== 'true') {
      const detail = repoCheck.message || ''
      const notRepo = /not a git repository/i.test(detail)
      const gitMissing = /ENOENT|not recognized|command not found/i.test(detail)
      return {
        vcs: '',
        isRepo: false,
        gitAvailable: !gitMissing,
        cwd,
        branch: '',
        hasHead: false,
        files: [],
        diff: '',
        diffTruncated: false,
        ahead: null,
        error: notRepo || gitMissing || !detail ? '' : detail,
      }
    }
    const [branchResult, headResult, statusResult] = await Promise.all([
      runGit(cwd, ['rev-parse', '--abbrev-ref', 'HEAD']),
      runGit(cwd, ['rev-parse', '--verify', 'HEAD']),
      runGit(cwd, ['status', '--porcelain', '--untracked-files=all']),
    ])
    const hasHead = headResult.ok
    const files = statusResult.ok ? parsePorcelainStatus(statusResult.stdout) : []
    let diff = ''
    let diffTruncated = false
    if (files.length) {
      const diffResult = await runGit(cwd, hasHead ? ['diff', 'HEAD'] : ['diff'])
      if (diffResult.ok) diff = diffResult.stdout
      for (const file of files.filter((item) => item.status === '??')) {
        if (diff.length >= MAX_DIFF_CHARS) break
        const untrackedDiff = await runGit(cwd, [
          'diff',
          '--no-index',
          '--',
          '/dev/null',
          file.path,
        ])
        if (untrackedDiff.stdout)
          diff += `${diff && !diff.endsWith('\n') ? '\n' : ''}${untrackedDiff.stdout}`
      }
      if (diff.length > MAX_DIFF_CHARS) {
        diff = diff.slice(0, MAX_DIFF_CHARS)
        diffTruncated = true
      }
    }
    let ahead = null
    if (hasHead) {
      const aheadResult = await runGit(cwd, ['rev-list', '--count', '@{upstream}..HEAD'])
      if (aheadResult.ok) ahead = Math.max(0, Number(aheadResult.stdout.trim()) || 0)
    }
    return {
      vcs: 'git',
      isRepo: true,
      gitAvailable: true,
      cwd,
      branch: branchResult.ok ? branchResult.stdout.trim() : '',
      hasHead,
      files,
      diff,
      diffTruncated,
      ahead,
    }
  }

  async commit(cwd, message) {
    const text = String(message || '').trim()
    if (!text) throw new Error('Commit message 不能为空。')
    if (text.length > MAX_COMMIT_MESSAGE_CHARS)
      throw new Error(`Commit message 不能超过 ${MAX_COMMIT_MESSAGE_CHARS} 个字符。`)
    const addResult = await runGit(cwd, ['add', '-A'])
    if (!addResult.ok) throw new Error(`git add 失败：${addResult.message}`)
    const commitResult = await runGit(cwd, ['commit', '-m', text])
    if (!commitResult.ok) throw new Error(`git commit 失败：${commitResult.message}`)
    return this.getChanges(cwd)
  }

  async push(cwd) {
    let pushResult = await runGit(cwd, ['push'], { timeout: PUSH_TIMEOUT_MS })
    if (!pushResult.ok && /no upstream|set-upstream|--set-upstream/i.test(pushResult.message)) {
      const branchResult = await runGit(cwd, ['rev-parse', '--abbrev-ref', 'HEAD'])
      const branch = branchResult.ok ? branchResult.stdout.trim() : ''
      if (branch && branch !== 'HEAD') {
        pushResult = await runGit(cwd, ['push', '--set-upstream', 'origin', branch], {
          timeout: PUSH_TIMEOUT_MS,
        })
      }
    }
    if (!pushResult.ok) throw new Error(`git push 失败：${pushResult.message}`)
    return this.getChanges(cwd)
  }

  async revert(cwd) {
    const headResult = await runGit(cwd, ['rev-parse', '--verify', 'HEAD'])
    if (headResult.ok) {
      const resetResult = await runGit(cwd, ['reset', '--hard', 'HEAD'])
      if (!resetResult.ok) throw new Error(`git reset 失败：${resetResult.message}`)
    }
    const cleanResult = await runGit(cwd, ['clean', '-fd'])
    if (!cleanResult.ok) throw new Error(`git clean 失败：${cleanResult.message}`)
    return this.getChanges(cwd)
  }
}
