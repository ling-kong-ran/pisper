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

// 运行模式影响进行中的 Goal/Team 轮次，因此运行中禁止切换。
test('run mode cannot switch while the session is running', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-mode-busy-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  runtime.sessions.set('session-1', { runActive: true })
  await assert.rejects(runtime.setSessionRunMode('session-1', 'plan'), /正在运行/)
  assert.equal(runtime.sessionMeta['session-1']?.runMode, undefined)
})

// 显式切回 Plan 必须暂停活动 Goal/Team：否则隐藏延续与团队快照会继续挂在会话上。
test('switching to plan pauses the active team goal and its workflow', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-mode-pause-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = new AgentRuntimeService({ cwd: directory, dataDir: directory })
  await runtime.goals.init()
  await runtime.teamWorkflows.init()
  runtime.sessions.set('session-1', {})
  const goal = await runtime.goals.start('session-1', {
    objective: 'Coordinate the team.',
    mode: 'team',
  })
  await runtime.teamWorkflows.ensure('session-1', {
    goalId: goal.id,
    objective: goal.objective,
  })
  const task = await runtime.teamWorkflows.registerTask('session-1', {
    taskName: 'inspect',
    message: 'Inspect.',
  })
  await runtime.teamWorkflows.bindAgent('session-1', task.id, { id: 'agent-1', status: 'running' })

  const result = await runtime.setSessionRunMode('session-1', 'plan')
  assert.equal(result.runMode, 'plan')
  assert.equal(runtime.goals.get('session-1').status, 'paused')
  const team = runtime.teamWorkflows.get('session-1')
  assert.equal(team.status, 'paused')
  assert.equal(team.tasks[0].status, 'interrupted')

  // 切回 team 并发送 team 消息后仍可恢复（目标不丢失）。
  await runtime.setSessionRunMode('session-1', 'team')
  const resumed = await runtime.goals.resume('session-1', { mode: 'team' })
  assert.equal(resumed.objective, 'Coordinate the team.')
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
  // 目标暂停（如用户切回 Plan）：团队属于历史轮次，不再随列表回灌。
  runtime.goals.state.goals['session-1'].status = 'paused'
  sessions = await runtime.listSessions()
  assert.equal(sessions.find((session) => session.id === 'session-1').team, null)
  // 预算受限仍投影：用户需要看到团队因预算停在哪里。
  runtime.goals.state.goals['session-1'].status = 'budget_limited'
  sessions = await runtime.listSessions()
  assert.ok(sessions.find((session) => session.id === 'session-1').team)
  // 目标完成后：列表投影变为 null 清除信号，面板不再残留。
  runtime.goals.state.goals['session-1'].status = 'complete'
  sessions = await runtime.listSessions()
  assert.equal(sessions.find((session) => session.id === 'session-1').team, null)
})

// 回归：/api/sessions/:id/live 轮询快照曾把 live.team 的 null 清除信号
// 被 ?? 运算符吞掉，回退到未受守卫的团队投影，导致 plan 轮次重新灌入「团队已完成」。
test('live snapshot respects the team clear signal and hides completed teams', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-run-mode-live-'))
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
  // 目标仍 active：无 live 状态时照常投影团队。
  let live = await runtime.getSessionLive('session-1')
  assert.ok(live.team)
  // plan 轮次的 live 状态带 team:null 清除信号：快照必须返回 null，
  // 不得回退到磁盘上的已完成团队记录。
  runtime.goals.state.goals['session-1'].status = 'complete'
  runtime.liveSessions.set('session-1', { team: null })
  live = await runtime.getSessionLive('session-1')
  assert.equal(live.team, null)
  // 会话运行时回收后（无 live）同样不投影已完成团队。
  runtime.liveSessions.delete('session-1')
  live = await runtime.getSessionLive('session-1')
  assert.equal(live.team, null)
})
