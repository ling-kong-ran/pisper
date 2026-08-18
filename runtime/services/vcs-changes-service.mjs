// VCS 变更服务：Git/SVN 统一封装——自动探测工作区类型并转发到对应实现。
import { GitChangesService } from './git-changes-service.mjs'
import { SvnChangesService } from './svn-changes-service.mjs'

const NOT_A_WORKSPACE_ERROR = '当前目录不是 Git/SVN 工作区。'

export class VcsChangesService {
  constructor({ git, svn } = {}) {
    this.git = git || new GitChangesService()
    this.svn = svn || new SvnChangesService()
  }

  async getChanges(cwd) {
    const gitChanges = await this.git.getChanges(cwd)
    if (gitChanges.isRepo) return { ...gitChanges, vcs: 'git' }
    const svnChanges = await this.svn.getChanges(cwd)
    if (svnChanges.isRepo) return { ...svnChanges, vcs: 'svn' }
    return {
      ...gitChanges,
      vcs: '',
      svnAvailable: svnChanges.svnAvailable,
      error: '',
    }
  }

  async detectVcs(cwd) {
    const changes = await this.getChanges(cwd)
    return changes.vcs || (changes.isRepo ? 'git' : '')
  }

  async commit(cwd, message) {
    const vcs = await this.detectVcs(cwd)
    if (vcs === 'svn') return this.svn.commit(cwd, message)
    if (vcs === 'git') return this.git.commit(cwd, message)
    throw new Error(NOT_A_WORKSPACE_ERROR)
  }

  async push(cwd) {
    const vcs = await this.detectVcs(cwd)
    if (vcs === 'svn') throw new Error('SVN 工作区无需推送，提交即同步到仓库。')
    if (vcs === 'git') return this.git.push(cwd)
    throw new Error(NOT_A_WORKSPACE_ERROR)
  }

  async revert(cwd) {
    const vcs = await this.detectVcs(cwd)
    if (vcs === 'svn') return this.svn.revert(cwd)
    if (vcs === 'git') return this.git.revert(cwd)
    throw new Error(NOT_A_WORKSPACE_ERROR)
  }
}
