import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'
import test from 'node:test'
import { GitChangesService } from '../services/git-changes-service.mjs'

const execFileAsync = promisify(execFile)

test('git changes include untracked files as green additions in the unified diff', async (t) => {
  try {
    await execFileAsync('git', ['--version'], { windowsHide: true })
  } catch {
    t.skip('Git is unavailable in this environment.')
    return
  }

  const cwd = await mkdtemp(join(tmpdir(), 'pisper-git-changes-'))
  try {
    await execFileAsync('git', ['init'], { cwd, windowsHide: true })
    await writeFile(join(cwd, 'new-file.txt'), 'first line\nsecond line\n', 'utf8')

    const changes = await new GitChangesService().getChanges(cwd)
    assert.equal(changes.isRepo, true)
    assert.deepEqual(changes.files, [{ path: 'new-file.txt', status: '??' }])
    assert.match(changes.diff, /\+\+\+ b\/new-file\.txt/)
    assert.match(changes.diff, /\+first line/)
    assert.match(changes.diff, /\+second line/)
  } finally {
    await rm(cwd, { recursive: true, force: true })
  }
})

test('git per-file diff covers tracked modifications, clean files and untracked new files', async (t) => {
  try {
    await execFileAsync('git', ['--version'], { windowsHide: true })
  } catch {
    t.skip('Git is unavailable in this environment.')
    return
  }

  const cwd = await mkdtemp(join(tmpdir(), 'pisper-git-file-diff-'))
  try {
    await execFileAsync('git', ['init'], { cwd, windowsHide: true })
    await execFileAsync('git', ['config', 'user.name', 'Pisper Test'], { cwd, windowsHide: true })
    await execFileAsync('git', ['config', 'user.email', 'test@pisper.local'], {
      cwd,
      windowsHide: true,
    })
    await writeFile(join(cwd, 'tracked.txt'), 'base line\n', 'utf8')
    await writeFile(join(cwd, 'clean.txt'), 'unchanged\n', 'utf8')
    await execFileAsync('git', ['add', '.'], { cwd, windowsHide: true })
    await execFileAsync('git', ['commit', '-m', 'base'], { cwd, windowsHide: true })
    await writeFile(join(cwd, 'tracked.txt'), 'base line\nmodified line\n', 'utf8')
    await writeFile(join(cwd, 'fresh.txt'), 'hello\n', 'utf8')

    const service = new GitChangesService()
    const modified = await service.getFileDiff(cwd, join(cwd, 'tracked.txt'))
    assert.equal(modified.isRepo, true)
    assert.match(modified.diff, /tracked\.txt/)
    assert.match(modified.diff, /\+modified line/)

    // 干净文件没有差异：前端据此提示「没有未提交的改动」而不是打开空 diff。
    const clean = await service.getFileDiff(cwd, join(cwd, 'clean.txt'))
    assert.equal(clean.isRepo, true)
    assert.equal(clean.diff, '')

    // 未跟踪的新文件 git diff 看不到：服务侧补一份全新增 diff。
    const fresh = await service.getFileDiff(cwd, join(cwd, 'fresh.txt'))
    assert.equal(fresh.isRepo, true)
    assert.match(fresh.diff, /--- \/dev\/null/)
    assert.match(fresh.diff, /\+hello/)
  } finally {
    await rm(cwd, { recursive: true, force: true })
  }
})

test('per-file diff reports non-repo directories as unavailable so the UI can fall back', async () => {
  const cwd = await mkdtemp(join(tmpdir(), 'pisper-git-file-diff-plain-'))
  try {
    await writeFile(join(cwd, 'loose.txt'), 'no vcs\n', 'utf8')
    const git = await new GitChangesService().getFileDiff(cwd, join(cwd, 'loose.txt'))
    assert.equal(git.isRepo, false)
    const vcs = await new (
      await import('../services/vcs-changes-service.mjs')
    ).VcsChangesService().getFileDiff(cwd, join(cwd, 'loose.txt'))
    assert.equal(vcs.isRepo, false)
    assert.equal(vcs.diff, '')
  } finally {
    await rm(cwd, { recursive: true, force: true })
  }
})

test('session file diff rejects paths outside the session workspace', async (t) => {
  const { AgentRuntimeService } = await import('../runtime/agent-runtime.mjs')
  const directory = await mkdtemp(join(tmpdir(), 'pisper-file-diff-scope-'))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  t.after(async () => {
    await runtime.dispose()
    await rm(directory, { recursive: true, force: true })
  })
  runtime.sessionWorkspaceCwd = async () => directory

  await assert.rejects(() => runtime.getSessionFileDiff('s1', join(directory, '..', 'out.txt')))
  await assert.rejects(() => runtime.getSessionFileDiff('s1', ''))
  // 工作区内路径正常放行；非仓库目录返回 isRepo: false 交给前端回退。
  const inside = await runtime.getSessionFileDiff('s1', join(directory, 'a.txt'))
  assert.equal(inside.isRepo, false)
})
