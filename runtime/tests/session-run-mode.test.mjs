import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { AgentRuntimeService } from '../runtime/agent-runtime.mjs'

// 回归守卫：PUT /api/sessions/:id/run-mode 直接调用 runtime.setSessionRunMode。
// 该方法曾只存在于 SessionLifecycle 而漏挂在服务方法清单上，
// 导致路由报「runtime.setSessionRunMode is not a function」。
test('runtime service exposes setSessionRunMode and persists the run mode per session', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-mode-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  assert.equal(typeof runtime.setSessionRunMode, 'function')

  runtime.sessions.set('session-1', {})
  // 非法模式直接拒绝；不存在的会话返回 null（路由层转 404）。
  await assert.rejects(runtime.setSessionRunMode('session-1', 'full-access'), /运行模式无效/)
  assert.equal(await runtime.setSessionRunMode('missing-session', 'team'), null)

  const updated = await runtime.setSessionRunMode('session-1', 'team')
  assert.deepEqual(updated, { id: 'session-1', runMode: 'team' })
  assert.equal(runtime.sessionMeta['session-1'].runMode, 'team')

  // 落盘持久化：冷启动后按同一文件恢复，刷新页面不再丢回 plan。
  const stored = JSON.parse(await readFile(join(directory, 'pisper-sessions.json'), 'utf8'))
  assert.equal(stored['session-1'].runMode, 'team')
})

// 会话列表投影必须携带 runMode，前端才能在刷新后首帧恢复执行模式。
test('session listing projects the persisted run mode with plan fallback', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-mode-list-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  // 最小配置桩：列表投影需要读取全局设置。
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  // 磁盘会话以「类型: session」头行的 jsonl 存在，此处造两个最小会话文件。
  await mkdir(runtime.sessionDir, { recursive: true })
  for (const id of ['session-1', 'session-2']) {
    await writeFile(
      join(runtime.sessionDir, `2026-01-01T00-00-00-000Z_${id}.jsonl`),
      `${JSON.stringify({ type: 'session', id, version: 3, timestamp: new Date().toISOString() })}\n`,
    )
  }
  runtime.sessionMeta['session-1'] = { runMode: 'goal' }
  const sessions = await runtime.listSessions()
  assert.equal(sessions.find((session) => session.id === 'session-1').runMode, 'goal')
  // 未设置过的会话回退默认 plan。
  assert.equal(sessions.find((session) => session.id === 'session-2').runMode, 'plan')
})

// 回归：团队目标完成后，会话列表不得再把团队快照回灌给客户端；
// 否则刷新或重新同步后「团队已完成」面板一直残留。
test('session listing stops projecting team snapshots once the goal is complete', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-mode-team-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  runtime.settingsManager = { getGlobalSettings: () => ({}) }
  await mkdir(runtime.sessionDir, { recursive: true })
  await writeFile(
    join(runtime.sessionDir, '2026-01-01T00-00-00-000Z_session-1.jsonl'),
    `${JSON.stringify({ type: 'session', id: 'session-1', version: 3, timestamp: new Date().toISOString() })}\n`,
  )
  const stamp = new Date().toISOString()
  await runtime.teamWorkflows.ensure('session-1', { objective: '团队目标' })
  runtime.goals.state.goals['session-1'] = {
    id: 'session-1',
    status: 'active',
    mode: 'team',
    objective: '团队目标',
    createdAt: stamp,
    updatedAt: stamp,
  }
  // 目标仍 active：团队照常投影。
  let sessions = await runtime.listSessions()
  assert.ok(sessions.find((session) => session.id === 'session-1').team)
  // 目标完成后：列表投影变为 null 清除信号，面板不再残留。
  runtime.goals.state.goals['session-1'].status = 'complete'
  sessions = await runtime.listSessions()
  assert.equal(sessions.find((session) => session.id === 'session-1').team, null)
})
