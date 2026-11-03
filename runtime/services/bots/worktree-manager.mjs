// worktree 管理（B2）：每个 Bot 任务在独立的 git worktree 与分支里改代码，用户的工作区在人确认前不被动到。
// 所有 git 调用都经执行器（Executor.exec），本机与以后的执行节点共用这套逻辑。
// worktree 放在 <数据目录>/worktrees/<仓库路径哈希>/<任务 id>，删除前确认路径在这个目录之下。
import { createHash } from 'node:crypto'
import { join, resolve, sep } from 'node:path'

export const MAX_WORKTREES_PER_REPO = 8
const MAX_DIFF_CHARS = 200_000
const MAX_COMMIT_MESSAGE_CHARS = 4_000
const FALLBACK_IDENTITY = ['-c', 'user.name=Pisper Bot', '-c', 'user.email=bot@pisper.local']

export class WorktreeError extends Error {
  constructor(code, message, statusCode = 400) {
    super(message)
    this.name = 'WorktreeError'
    this.code = code
    this.statusCode = statusCode
  }
}

function repoKey(repoRoot) {
  return createHash('sha256').update(resolve(repoRoot)).digest('hex').slice(0, 12)
}

// 分支名里的 Bot 段：保留文字（含中文）、数字与连字符，其余换成连字符。
export function botBranchSegment(name) {
  const segment = String(name || '')
    .normalize('NFKC')
    .toLowerCase()
    .replace(/[^\p{L}\p{N}-]+/gu, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 24)
  return segment || 'bot'
}

export class WorktreeManager {
  constructor({ executor, root }) {
    this.executor = executor
    this.root = resolve(root)
  }

  async #git(args, cwd, options) {
    return this.executor.exec('git', args, cwd, options)
  }

  async #gitOk(args, cwd, failure) {
    const result = await this.#git(args, cwd)
    if (result.code !== 0)
      throw new WorktreeError(
        failure.code,
        `${failure.message}${result.stderr.trim() ? `：${result.stderr.trim().slice(0, 300)}` : ''}`,
      )
    return result.stdout
  }

  // 仓库没配置提交身份时，用 Bot 自己的身份提交，避免任务卡在“Please tell me who you are”。
  async #identity(cwd) {
    const result = await this.#git(['config', 'user.email'], cwd)
    return result.code === 0 && result.stdout.trim() ? [] : FALLBACK_IDENTITY
  }

  #within(path) {
    const target = resolve(path)
    return target.startsWith(`${this.root}${sep}`)
  }

  async create({ cwd, taskId, botName }) {
    const top = await this.#git(['rev-parse', '--show-toplevel'], cwd)
    if (top.code !== 0)
      throw new WorktreeError(
        'workspace_not_git',
        '工作目录不在 git 仓库里，无法为任务创建独立分支。',
      )
    const repoRoot = top.stdout.trim()
    const baseRef = (
      await this.#gitOk(['rev-parse', 'HEAD'], repoRoot, {
        code: 'workspace_not_git',
        message: '仓库还没有提交，无法创建独立分支',
      })
    ).trim()
    const repoDir = join(this.root, repoKey(repoRoot))
    const listed = await this.#gitOk(['worktree', 'list', '--porcelain'], repoRoot, {
      code: 'worktree_failed',
      message: '读取 worktree 列表失败',
    })
    const mine = listed
      .split('\n')
      .filter((line) => line.startsWith('worktree '))
      .map((line) => resolve(line.slice('worktree '.length)))
      .filter((path) => path.startsWith(`${repoDir}${sep}`))
    if (mine.length >= MAX_WORKTREES_PER_REPO)
      throw new WorktreeError(
        'worktree_limit',
        `这个仓库已经有 ${MAX_WORKTREES_PER_REPO} 个待处理的任务分支，先接受或放弃一些。`,
        409,
      )
    const path = join(repoDir, taskId)
    const branch = `pisper/${botBranchSegment(botName)}/${taskId.slice(-8)}`
    await this.#gitOk(['check-ref-format', '--branch', branch], repoRoot, {
      code: 'worktree_failed',
      message: '分支名无效',
    })
    await this.#gitOk(['worktree', 'add', '-b', branch, path, baseRef], repoRoot, {
      code: 'worktree_failed',
      message: '创建 worktree 失败',
    })
    return { kind: 'worktree', repoRoot, path, branch, baseRef, state: 'ready' }
  }

  // 汇总改动：把新文件也纳入暂存区再与基线比较，未跟踪文件同样出现在 diff 里。
  async changes(workspace) {
    await this.#gitOk(['add', '-A'], workspace.path, {
      code: 'worktree_failed',
      message: '汇总改动失败',
    })
    const names = await this.#gitOk(
      ['diff', '--cached', '--name-status', workspace.baseRef],
      workspace.path,
      { code: 'worktree_failed', message: '汇总改动失败' },
    )
    const files = names
      .split('\n')
      .filter(Boolean)
      .map((line) => {
        const [status, ...rest] = line.split('\t')
        return { status: status.charAt(0), path: rest.at(-1) }
      })
    const diff = await this.#gitOk(['diff', '--cached', workspace.baseRef], workspace.path, {
      code: 'worktree_failed',
      message: '读取改动失败',
    })
    return {
      files,
      diff: diff.slice(0, MAX_DIFF_CHARS),
      truncated: diff.length > MAX_DIFF_CHARS,
    }
  }

  // 在任务分支上提交；没有改动时返回 null。
  async commit(workspace, message) {
    await this.#gitOk(['add', '-A'], workspace.path, {
      code: 'worktree_failed',
      message: '提交失败',
    })
    const staged = await this.#git(['diff', '--cached', '--quiet'], workspace.path)
    if (staged.code === 0) {
      const head = await this.#gitOk(['rev-parse', 'HEAD'], workspace.path, {
        code: 'worktree_failed',
        message: '提交失败',
      })
      return head.trim() === workspace.baseRef ? null : head.trim()
    }
    const text = String(message || 'Bot 任务').slice(0, MAX_COMMIT_MESSAGE_CHARS)
    await this.#gitOk(
      [...(await this.#identity(workspace.path)), 'commit', '--no-verify', '-m', text],
      workspace.path,
      { code: 'worktree_failed', message: '提交失败' },
    )
    return (
      await this.#gitOk(['rev-parse', 'HEAD'], workspace.path, {
        code: 'worktree_failed',
        message: '提交失败',
      })
    ).trim()
  }

  // 应用到用户当前分支：只用 cherry-pick（不 merge）；主工作区不干净或有冲突就放弃，分支留给人处理。
  async apply(workspace, commit) {
    const status = await this.#gitOk(['status', '--porcelain'], workspace.repoRoot, {
      code: 'worktree_failed',
      message: '读取仓库状态失败',
    })
    if (status.trim())
      throw new WorktreeError('repo_dirty', '当前分支有未提交的改动，先处理后再应用。', 409)
    const picked = await this.#git(
      [...(await this.#identity(workspace.repoRoot)), 'cherry-pick', commit],
      workspace.repoRoot,
    )
    if (picked.code !== 0) {
      await this.#git(['cherry-pick', '--abort'], workspace.repoRoot)
      throw new WorktreeError(
        'apply_conflict',
        `应用时有冲突，已撤回；改动仍在分支 ${workspace.branch} 上。`,
        409,
      )
    }
  }

  async remove(workspace, { deleteBranch = false } = {}) {
    if (!workspace || workspace.kind !== 'worktree' || workspace.state === 'removed') return
    if (!this.#within(workspace.path))
      throw new WorktreeError('worktree_outside_root', '拒绝删除 Pisper worktree 目录之外的路径。')
    await this.#git(['worktree', 'remove', '--force', workspace.path], workspace.repoRoot)
    if (deleteBranch) await this.#git(['branch', '-D', workspace.branch], workspace.repoRoot)
    await this.#git(['worktree', 'prune'], workspace.repoRoot)
  }
}
