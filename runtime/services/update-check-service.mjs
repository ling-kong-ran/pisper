// 更新检查服务：对比当前版本/提交与远端 release 分支，判断是否有新版本可更新。
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import {
  DEFAULT_BRANCH,
  normalizedVersion,
  REPOSITORY_API,
  REPOSITORY_URL,
} from '../../shared/app-update.mjs'

// Update checks always compare against the current remote branch. Only an in-flight request is
// shared, so concurrent callers do not duplicate network work without retaining stale results.
const execFileAsync = promisify(execFile)

function validCommit(value) {
  const commit = String(value || '')
    .trim()
    .toLowerCase()
  return /^[0-9a-f]{7,40}$/.test(commit) ? commit : ''
}

export async function resolveGitCommit(root, { env = process.env, runGit = execFileAsync } = {}) {
  const configured = validCommit(env.PISPER_COMMIT_SHA || env.GITHUB_SHA)
  if (configured) return configured
  try {
    const result = await runGit('git', ['-C', root, 'rev-parse', 'HEAD'], {
      encoding: 'utf8',
      windowsHide: true,
    })
    return validCommit(result.stdout)
  } catch {
    return ''
  }
}

function commitNotes(commits = [], branch = DEFAULT_BRANCH) {
  const lines = commits.slice(-20).map((item) => {
    const title =
      String(item.commit?.message || '')
        .split(/\r?\n/)[0]
        .trim() || 'Untitled commit'
    return `- ${title} (${String(item.sha || '').slice(0, 7)})`
  })
  return lines.length ? `## ${branch} 分支更新\n\n${lines.join('\n')}` : ''
}

export class UpdateCheckService {
  constructor({
    currentVersion,
    currentCommit,
    branch = DEFAULT_BRANCH,
    fetcher = fetch,
    now = () => Date.now(),
  } = {}) {
    this.currentVersion = normalizedVersion(currentVersion)
    this.currentCommit = validCommit(currentCommit)
    this.branch = branch
    this.fetcher = fetcher
    this.now = now
    this.pending = null
  }

  async check(_options = {}) {
    if (this.pending) return this.pending
    this.pending = this.fetchLatest().finally(() => {
      this.pending = null
    })
    return this.pending
  }

  async fetchLatest() {
    if (!this.currentCommit)
      throw new Error(
        '无法识别当前 Web 源码的 Git commit。请使用 Git 仓库运行，或设置 PISPER_COMMIT_SHA。',
      )
    const response = await this.fetcher(
      `${REPOSITORY_API}/compare/${this.currentCommit}...${encodeURIComponent(this.branch)}`,
      {
        headers: {
          Accept: 'application/vnd.github+json',
          'X-GitHub-Api-Version': '2022-11-28',
          'User-Agent': `Pisper/${this.currentVersion || 'web'} (${this.currentCommit.slice(0, 7)})`,
        },
      },
    )
    if (!response.ok) throw new Error(`GitHub commit 比较失败：HTTP ${response.status}`)
    const comparison = await response.json()
    const commits = Array.isArray(comparison.commits) ? comparison.commits : []
    const aheadBy = Math.max(0, Number(comparison.ahead_by) || 0)
    const available = aheadBy > 0
    const latest = commits.at(-1) || comparison.base_commit || {}
    const value = {
      state: available ? 'available' : 'current',
      currentVersion: this.currentVersion,
      currentCommit: this.currentCommit,
      availableCommit: validCommit(latest.sha),
      behindBy: aheadBy,
      releaseDate: latest.commit?.committer?.date || latest.commit?.author?.date || null,
      branch: this.branch,
      notes: commitNotes(commits, this.branch),
      releaseUrl: comparison.html_url || `${REPOSITORY_URL}/commits/${this.branch}`,
      canDownload: false,
      checkedAt: new Date(this.now()).toISOString(),
      message: available
        ? `Web 源码落后 ${this.branch} ${aheadBy} 个提交，请查看更新内容后自行更新。`
        : `当前 Web 源码已同步 ${this.branch}。`,
    }
    return value
  }
}
