import assert from 'node:assert/strict'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { createMultiAgentRuntime } from '../runtime/multi-agent-runtime-adapter.mjs'
import {
  compactTeamProjection,
  TEAM_TASK_LEASE_MS,
  TeamWorkflowService,
  TEAM_EXECUTION_MARKER,
  teamExecutionPrompt,
} from '../services/team-workflow.mjs'

async function waitFor(predicate, message) {
  const deadline = Date.now() + 2_000
  while (Date.now() < deadline) {
    if (predicate()) return
    await new Promise((resolve) => setTimeout(resolve, 5))
  }
  throw new Error(`Timed out waiting for ${message}.`)
}

test('team execution prompt defines dynamic delegation and evidence-based convergence', () => {
  const prompt = teamExecutionPrompt({ objective: 'Complete the project and verify it.' }, [
    { canonicalName: '/root/architect_1', status: 'completed' },
    { canonicalName: '/root/tester_2', status: 'running' },
  ])

  assert.ok(prompt.startsWith(TEAM_EXECUTION_MARKER))
  assert.match(prompt, /dynamic software engineering team/i)
  assert.match(prompt, /non-overlapping workstreams/i)
  assert.match(prompt, /single direct task is valid/i)
  assert.match(prompt, /completion audit/i)
  assert.match(prompt, /architect_1: completed/)
  assert.match(prompt, /tester_2: running/)
})

test('team workflow enforces dependencies, active file ownership, and completion barriers', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-workflow-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  const team = await service.ensure('session-1', {
    goalId: 'goal-1',
    objective: 'Ship the project.',
    tokenBudget: 10_000,
  })

  const architect = await service.registerTask('session-1', {
    taskName: 'architect',
    role: 'architect',
    files: ['src/api.ts'],
    message: 'Inspect the API boundary.',
  })
  const tester = await service.registerTask('session-1', {
    taskName: 'tester',
    role: 'tester',
    files: ['tests/api.test.ts'],
    message: 'Prepare focused tests.',
  })
  assert.equal(team.taskCount, 0)
  assert.equal(architect.status, 'queued')
  assert.equal(tester.status, 'queued')

  await assert.rejects(
    service.registerTask('session-1', {
      taskName: 'api-implementer',
      files: ['src/api.ts'],
      message: 'Implement the API.',
    }),
    /file ownership conflict/i,
  )
  await service.bindAgent('session-1', architect.id, { id: 'agent-1', status: 'running' })
  const reviewer = await service.registerTask('session-1', {
    taskName: 'api-reviewer',
    role: 'reviewer',
    dependsOn: ['architect'],
    files: ['src/api.ts'],
    message: 'Review the API after implementation.',
  })
  assert.equal(reviewer.status, 'blocked')
  await service.updateAgent('session-1', {
    id: 'agent-1',
    status: 'completed',
    output: 'API boundary verified.',
  })
  assert.equal(
    service.get('session-1').tasks.find((task) => task.id === reviewer.id).status,
    'queued',
  )
  await service.bindAgent('session-1', reviewer.id, { id: 'agent-3', status: 'running' })
  assert.equal(service.canComplete('session-1').ok, false)
  await service.updateAgent('session-1', {
    id: 'agent-3',
    status: 'completed',
    output: 'API review passed.',
  })
  assert.equal(service.canComplete('session-1').ok, false)

  await service.bindAgent('session-1', tester.id, { id: 'agent-2', status: 'running' })
  await service.updateAgent('session-1', {
    id: 'agent-2',
    status: 'completed',
  })
  assert.match(service.canComplete('session-1').reason, /缺少可验证结果/)
  await service.updateAgent('session-1', {
    id: 'agent-2',
    status: 'completed',
    output: 'Focused tests passed.',
  })
  const completion = service.canComplete('session-1')
  assert.equal(completion.ok, true)
  assert.equal(completion.team.completedTaskCount, 3)
  assert.equal((await service.markComplete('session-1')).status, 'complete')

  const restored = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await restored.init()
  assert.equal(restored.get('session-1').status, 'complete')
  assert.equal(restored.get('session-1').taskCount, 3)
})

test('team workflow supports repeated recovery without time-based termination', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-limits-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let clock = Date.parse('2025-01-01T00:00:00.000Z')
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json'), now: () => clock })
  await service.init()
  await service.ensure('session-1', { goalId: 'goal-1', objective: 'Finish safely.' })

  const settled = await service.registerTask('session-1', { taskName: 'settled' })
  await service.bindAgent('session-1', settled.id, {
    id: 'agent-1',
    status: 'completed',
    output: 'Already complete.',
  })
  assert.equal(service.get('session-1').tasks[0].status, 'completed')

  await service.updateAgent('session-1', { id: 'agent-1', status: 'failed', error: 'Transient.' })
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    await service.prepareRetry('session-1', { id: 'agent-1' })
    assert.equal(service.get('session-1').tasks[0].attempts, attempt + 1)
    await service.updateAgent('session-1', { id: 'agent-1', status: 'failed', error: 'Transient.' })
  }
  assert.equal(service.get('session-1').tasks[0].status, 'failed')
  assert.equal(service.refreshLimits('session-1'), null)
  assert.equal(service.get('session-1').status, 'active')

  const leaseTask = await service.registerTask('session-1', { taskName: 'lease-recovery' })
  const claimed = await service.claimTask('session-1', leaseTask.id, {
    leaseMs: TEAM_TASK_LEASE_MS,
  })
  assert.equal(claimed.status, 'starting')
  clock += TEAM_TASK_LEASE_MS
  assert.equal(service.taskReady('session-1', leaseTask.id), false)
  const [expired] = await service.requeueExpiredLeases('session-1')
  assert.equal(expired.taskId, leaseTask.id)
  assert.equal(service.taskReady('session-1', leaseTask.id), true)
  assert.equal(service.getTask('session-1', leaseTask.id).status, 'queued')
  await service.write
})

test('expired Team leases fence stale workers and retain full completion evidence', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-lease-fence-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let clock = Date.parse('2025-01-01T00:00:00.000Z')
  const service = new TeamWorkflowService({
    path: join(directory, 'teams.json'),
    now: () => clock,
  })
  await service.init()
  await service.ensure('session-1', { objective: 'Fence stale work.' })
  const task = await service.registerTask('session-1', { taskName: 'inspect' })
  const firstClaim = await service.claimTask('session-1', task.id)
  await service.bindAgent(
    'session-1',
    task.id,
    { id: 'agent-old', status: 'running' },
    { leaseId: firstClaim.leaseId },
  )

  clock += TEAM_TASK_LEASE_MS
  const [expired] = await service.requeueExpiredLeases('session-1')
  assert.equal(expired.agentId, 'agent-old')
  assert.equal(expired.leaseId, firstClaim.leaseId)
  const secondClaim = await service.claimTask('session-1', task.id)
  assert.notEqual(secondClaim.leaseId, firstClaim.leaseId)
  await service.bindAgent(
    'session-1',
    task.id,
    { id: 'agent-new', status: 'running' },
    { leaseId: secondClaim.leaseId },
  )

  assert.equal(
    await service.updateLeasedAgent('session-1', task.id, firstClaim.leaseId, {
      id: 'agent-old',
      status: 'completed',
      output: 'Stale result.',
    }),
    null,
  )
  assert.equal(service.getTask('session-1', task.id).agentId, 'agent-new')
  await service.recordCommunication('session-1', {
    fromAgentId: 'lead',
    toAgentId: 'agent-new',
    message: 'Queued handoff.',
    status: 'queued',
  })
  const delivered = await service.markCommunicationsDelivered('session-1', 'agent-new')
  assert.equal(delivered.length, 1)
  assert.equal(service.get('session-1').communications[0].status, 'delivered')
  const evidence = 'x'.repeat(10_000)
  await service.updateLeasedAgent('session-1', task.id, secondClaim.leaseId, {
    id: 'agent-new',
    status: 'completed',
    output: evidence,
  })
  await service.setSummary('session-1', evidence)
  const full = service.get('session-1')
  assert.equal(full.tasks[0].output.length, 10_000)
  assert.equal(full.summary.text.length, 10_000)
  const compact = compactTeamProjection(full)
  assert.equal(compact.tasks[0].output.length, 500)
  assert.equal(compact.summary.text.length, 4_000)

  const maximal = compactTeamProjection({
    ...full,
    tasks: Array.from({ length: 64 }, (_, index) => ({
      ...full.tasks[0],
      id: `task-${index}`,
      taskName: `task-${index}`,
      message: 'm'.repeat(12_000),
      files: Array.from({ length: 32 }, () => 'f'.repeat(512)),
      dependsOn: Array.from({ length: 32 }, () => 'dependency'.repeat(5)),
      blockedReason: 'b'.repeat(1_000),
      output: evidence,
      error: 'e'.repeat(1_000),
    })),
    communications: Array.from({ length: 32 }, (_, index) => ({
      id: `communication-${index}`,
      message: 'c'.repeat(12_000),
    })),
    conflicts: Array.from({ length: 24 }, () => ({
      files: Array.from({ length: 32 }, () => 'f'.repeat(512)),
      message: 'c'.repeat(1_000),
    })),
  })
  assert.equal(Object.hasOwn(maximal.tasks[0], 'files'), false)
  assert.ok(Buffer.byteLength(JSON.stringify(maximal), 'utf8') < 250_000)
})

test('team workflow restores an interrupted prerequisite without stranding blocked dependents', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-restart-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'teams.json')
  const service = new TeamWorkflowService({ path })
  await service.init()
  await service.ensure('session-1', { goalId: 'goal-1', objective: 'Recover the team.' })
  const prerequisite = await service.registerTask('session-1', { taskName: 'inspect' })
  await service.registerTask('session-1', {
    taskName: 'apply',
    dependsOn: ['inspect'],
  })
  const claimed = await service.claimTask('session-1', prerequisite.id)
  await service.bindAgent(
    'session-1',
    prerequisite.id,
    { id: 'agent-1', status: 'running' },
    { leaseId: claimed.leaseId },
  )

  const restored = new TeamWorkflowService({ path })
  await restored.init({ pauseActive: true })
  assert.equal(restored.get('session-1').status, 'paused')
  assert.deepEqual(
    restored.get('session-1').tasks.map((task) => [task.taskName, task.status]),
    [
      ['inspect', 'interrupted'],
      ['apply', 'blocked'],
    ],
  )
  await restored.ensure('session-1', { goalId: 'goal-1', objective: 'Recover the team.' })
  assert.deepEqual(
    restored.get('session-1').tasks.map((task) => [task.taskName, task.status]),
    [
      ['inspect', 'queued'],
      ['apply', 'blocked'],
    ],
  )
  const recovered = await restored.claimTask('session-1', prerequisite.id)
  await restored.bindAgent(
    'session-1',
    prerequisite.id,
    { id: 'agent-2', status: 'completed', output: 'Inspection recovered.' },
    { leaseId: recovered.leaseId },
  )
  assert.equal(restored.findTask('session-1', 'apply').status, 'queued')
})

test('team task graph can be revised and keeps role labels optional', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-graph-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { objective: 'Adapt the workflow.' })
  const inspect = await service.registerTask('session-1', {
    taskName: 'inspect',
    files: ['src/'],
    message: 'Inspect the implementation.',
  })
  const followup = await service.registerTask('session-1', {
    taskName: 'followup',
    dependsOn: ['inspect'],
    message: 'Apply the findings.',
  })
  assert.equal(inspect.role, '')
  assert.equal(followup.status, 'blocked')
  const updated = await service.updateTask('session-1', followup.id, {
    taskName: 'apply_findings',
    role: 'implementer',
    files: ['runtime/'],
    message: 'Apply the verified findings.',
  })
  assert.equal(updated.taskName, 'apply_findings')
  assert.equal(updated.status, 'blocked')
  await assert.rejects(
    service.updateTask('session-1', inspect.id, { dependsOn: ['apply_findings'] }),
    /dependency cycle/i,
  )
  await assert.rejects(
    service.updateTask('session-1', inspect.id, { dependsOn: ['inspect'] }),
    /cannot depend on itself/i,
  )
})

test('team adapter automatically starts a pending dependent task after its prerequisite finishes', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-scheduler-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { goalId: 'goal-1', objective: 'Coordinate dependent work.' })
  const spawned = []
  const multiAgents = {
    async spawn(input) {
      spawned.push(input)
      return {
        id: `agent-${spawned.length}`,
        canonicalName: `/root/${input.taskName}_${spawned.length}`,
        status: 'running',
        output: '',
      }
    },
    list: () => [],
    find: () => null,
    sendMessage: async () => null,
    followup: async () => null,
    interrupt: () => null,
  }
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: directory,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    getToolRisk: () => null,
    createInheritedCustomTools: () => [],
    waitAgent: async () => ({ timedOut: false, agents: [], agent: null }),
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: () => {},
  })

  const first = await runtime.spawn({ taskName: 'inspect', message: 'Inspect the project.' })
  const second = await runtime.spawn({
    taskName: 'apply',
    dependsOn: ['inspect'],
    message: 'Apply the findings.',
  })
  assert.equal(first.status, 'running')
  assert.equal(second.status, 'queued')
  assert.equal(spawned.length, 1)
  await spawned[0].onTerminal({ id: first.id, status: 'completed', output: 'Findings ready.' })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(spawned.length, 2)
  assert.equal(
    service.get('session-1').tasks.find((task) => task.taskName === 'apply').status,
    'running',
  )
})

test('team adapter interrupts an expired worker and ignores its late terminal callback', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-expired-worker-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  let clock = Date.parse('2025-01-01T00:00:00.000Z')
  const service = new TeamWorkflowService({
    path: join(directory, 'teams.json'),
    now: () => clock,
  })
  await service.init()
  await service.ensure('session-1', { objective: 'Recover an expired worker.' })
  const spawned = []
  const interrupted = []
  const multiAgents = {
    async spawn(input) {
      const agent = {
        id: `agent-${spawned.length + 1}`,
        taskName: input.taskName,
        canonicalName: `/root/${input.taskName}_${spawned.length + 1}`,
        status: 'running',
        output: '',
      }
      spawned.push({ ...input, agent })
      return agent
    },
    list: () => spawned.map((entry) => entry.agent),
    find: (_sessionId, target) => spawned.find((entry) => entry.agent.id === target)?.agent || null,
    sendMessage: async () => null,
    followup: async () => null,
    interrupt: (_sessionId, target) => interrupted.push(target),
  }
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: directory,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    getToolRisk: () => null,
    createInheritedCustomTools: () => [],
    waitAgent: async () => ({ timedOut: false, agent: null }),
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: () => {},
  })

  await runtime.spawn({ taskName: 'inspect', message: 'Inspect once.' })
  const firstLease = service.findTask('session-1', 'inspect').leaseId
  clock += TEAM_TASK_LEASE_MS
  await runtime.resume()
  assert.deepEqual(interrupted, ['agent-1'])
  assert.equal(spawned.length, 2)
  assert.notEqual(service.findTask('session-1', 'inspect').leaseId, firstLease)

  await spawned[0].onTerminal({
    id: 'agent-1',
    status: 'completed',
    output: 'Late stale output.',
  })
  assert.equal(service.findTask('session-1', 'inspect').status, 'running')
  assert.equal(service.findTask('session-1', 'inspect').agentId, 'agent-2')
  await spawned[1].onTerminal({
    id: 'agent-2',
    status: 'completed',
    output: 'Fresh output.',
  })
  assert.equal(service.findTask('session-1', 'inspect').output, 'Fresh output.')
})

test('Team follow-up run numbers cannot reuse a newer lease from the same Agent id', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-followup-fence-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { objective: 'Fence follow-up generations.' })
  let callbacks = null
  let record = null
  const multiAgents = {
    async spawn(input) {
      callbacks = input
      record = {
        id: 'agent-1',
        taskName: input.taskName,
        canonicalName: '/root/inspect_1',
        status: 'running',
        runNumber: 1,
        output: '',
      }
      return record
    },
    list: () => (record ? [record] : []),
    find: () => record,
    sendMessage: async () => null,
    async followup() {
      record = { ...record, status: 'queued', runNumber: 1, output: '' }
      return record
    },
    interrupt: () => null,
  }
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: directory,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    getToolRisk: () => null,
    createInheritedCustomTools: () => [],
    waitAgent: async () => ({ timedOut: false, agent: null }),
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: () => {},
  })

  await runtime.spawn({ taskName: 'inspect', message: 'Inspect once.' })
  const firstLease = service.findTask('session-1', 'inspect').leaseId
  await callbacks.onTerminal({
    id: 'agent-1',
    status: 'completed',
    runNumber: 1,
    output: 'First result.',
  })
  record = { ...record, status: 'completed', runNumber: 1, output: 'First result.' }
  await runtime.followup('agent-1', 'Inspect again.')
  const secondLease = service.findTask('session-1', 'inspect').leaseId
  assert.notEqual(secondLease, firstLease)

  callbacks.onProgress({ id: 'agent-1', status: 'running', runNumber: 2, output: '' })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(service.findTask('session-1', 'inspect').status, 'running')
  callbacks.onProgress({
    id: 'agent-1',
    status: 'completed',
    runNumber: 1,
    output: 'Late first result.',
  })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(service.findTask('session-1', 'inspect').status, 'running')
  await callbacks.onTerminal({
    id: 'agent-1',
    status: 'completed',
    runNumber: 2,
    output: 'Second result.',
  })
  assert.equal(service.findTask('session-1', 'inspect').output, 'Second result.')
})

test('team member tools send direct messages and expose the shared roster', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-messaging-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { objective: 'Coordinate member handoffs.' })
  const records = new Map()
  const spawned = []
  const events = []
  const find = (target) =>
    [...records.values()].find(
      (agent) => agent.id === target || agent.taskName === target || agent.canonicalName === target,
    ) || null
  const multiAgents = {
    async spawn(input) {
      const id = `agent-${spawned.length + 1}`
      const agent = {
        id,
        taskName: input.taskName,
        canonicalName: `/root/${input.taskName}_${spawned.length + 1}`,
        status: 'running',
        output: '',
        currentActivity: null,
      }
      spawned.push({ ...input, id })
      records.set(id, agent)
      return agent
    },
    list: () => [...records.values()],
    find: (_sessionId, target) => find(target),
    sendMessageFromAgent: async (_sessionId, _senderId, target, message) => {
      const agent = find(target)
      agent.lastMessage = message
      return agent
    },
    sendMessage: async (_sessionId, target) => find(target),
    followup: async () => null,
    interrupt: () => null,
  }
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: directory,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    getToolRisk: () => null,
    createInheritedCustomTools: () => [],
    waitAgent: async () => ({ timedOut: false, agents: [], agent: null }),
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: (_sessionId, agent) => events.push(agent),
  })

  await runtime.spawn({ taskName: 'research', message: 'Collect evidence.' })
  await runtime.spawn({ taskName: 'review', message: 'Review evidence.' })
  const childTools = await spawned[0].createCustomTools({ id: spawned[0].id })
  const sendTool = childTools.tools.find((tool) => tool.name === 'send_team_message')
  const listTool = childTools.tools.find((tool) => tool.name === 'list_team_members')
  const sent = await sendTool.execute('message-1', {
    target: 'review',
    message: 'The evidence is ready for review.',
  })
  const listed = await listTool.execute('list-1', {})

  assert.equal(sent.details.agent.taskName, 'review')
  assert.equal(sent.details.communication.fromTaskName, 'research')
  assert.equal(sent.details.communication.toTaskName, 'review')
  assert.equal(records.get(spawned[1].id).lastMessage, 'The evidence is ready for review.')
  assert.deepEqual(
    listed.details.members.map((member) => member.taskName),
    ['research', 'review'],
  )
  assert.equal(service.get('session-1').communications.length, 1)
  assert.equal(events.at(-1).communication.message, 'The evidence is ready for review.')

  const restored = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await restored.init()
  assert.equal(restored.get('session-1').communications[0].fromTaskName, 'research')
})

test('a dynamic JavaScript Team workflow fans out from intermediate results and persists its tasks', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-script-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { objective: 'Run scripted Team coordination.' })
  const spawned = []
  const records = new Map()
  const terminalWaiters = new Map()
  const find = (target) =>
    [...records.values()].find(
      (agent) => agent.id === target || agent.taskName === target || agent.canonicalName === target,
    ) || null
  const multiAgents = {
    async spawn(input) {
      const id = `agent-${spawned.length + 1}`
      const agent = {
        id,
        taskName: input.taskName,
        canonicalName: `/root/${input.taskName}_${spawned.length + 1}`,
        status: 'running',
        output: '',
      }
      spawned.push({ ...input, id })
      records.set(id, agent)
      return agent
    },
    list: () => [...records.values()],
    find: (_sessionId, target) => find(target),
    sendMessageFromAgent: async (_sessionId, _senderId, target) => find(target),
    sendMessage: async (_sessionId, target) => find(target),
    followup: async () => null,
    interrupt: () => null,
  }
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: directory,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    createInheritedCustomTools: () => [],
    waitAgent: async (_timeoutMs, target) => {
      const agent = find(target)
      if (agent?.status === 'completed') return { timedOut: false, agent }
      return new Promise((resolve) => {
        const waiters = terminalWaiters.get(target) || []
        waiters.push(resolve)
        terminalWaiters.set(target, waiters)
      })
    },
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: () => {},
  })
  await writeFile(
    join(directory, 'team.js'),
    `export const meta = {\n  name: 'workspace-review',\n  description: 'Inspect and verify a workspace review',\n}\n\nphase('inspect')\nconst inspection = await agent(\`Inspect the workspace for: \${args.target}\`, {\n  label: 'inspect',\n  role: 'investigator',\n  files: ['evidence.txt'],\n  schema: { type: 'object', required: ['files'], properties: { files: { type: 'array' } } },\n})\nphase('verify')\nconst verification = await pipeline(inspection.files, file =>\n  agent('Verify this evidence file: ' + file, {\n    label: 'verify_' + file,\n    role: 'reviewer',\n    files: [file],\n  }),\n)\nreturn { inspection, verification }\n`,
  )

  const runPromise = runtime.runScript('team.js', { target: 'current changes' })
  await waitFor(() => spawned.length === 1, 'the first workflow agent to start')
  assert.equal(spawned.length, 1)
  assert.match(spawned[0].message, /current changes/)
  assert.equal(service.get('session-1').scriptPath, 'team.js')
  assert.equal(service.get('session-1').tasks[0].autoStart, true)

  const inspectionOutput = JSON.stringify({ files: ['first.txt', 'second.txt'] })
  records.get(spawned[0].id).status = 'completed'
  records.get(spawned[0].id).output = inspectionOutput
  await spawned[0].onTerminal({
    id: spawned[0].id,
    status: 'completed',
    output: inspectionOutput,
  })
  for (const resolve of terminalWaiters.get(spawned[0].id) || [])
    resolve({ timedOut: false, agent: records.get(spawned[0].id) })
  await waitFor(() => spawned.length === 3, 'the verification workflow agents to start')
  assert.match(spawned[1].message, /first\.txt/)
  assert.match(spawned[2].message, /second\.txt/)

  for (const [index, entry] of spawned.slice(1).entries()) {
    const output = `Verification ${index + 1} done.`
    records.get(entry.id).status = 'completed'
    records.get(entry.id).output = output
    await entry.onTerminal({ id: entry.id, status: 'completed', output })
    for (const resolve of terminalWaiters.get(entry.id) || [])
      resolve({ timedOut: false, agent: records.get(entry.id) })
  }
  const result = await runPromise
  assert.equal(result.meta.name, 'workspace-review')
  assert.deepEqual(result.result, {
    inspection: { files: ['first.txt', 'second.txt'] },
    verification: ['Verification 1 done.', 'Verification 2 done.'],
  })
  assert.equal(result.taskCount, 3)

  const resumed = await runtime.runScript('team.js', { target: 'current changes' })
  assert.equal(spawned.length, 3)
  assert.equal(resumed.taskCount, 3)
  assert.deepEqual(resumed.result, result.result)

  const previousFingerprint = service.findTask('session-1', 'inspect_inspect_1').workflowFingerprint
  const changedPromise = runtime.runScript('team.js', { target: 'different changes' })
  await waitFor(() => spawned.length === 4, 'the changed-input inspection to restart')
  assert.match(spawned[3].message, /different changes/)
  records.get(spawned[3].id).status = 'completed'
  records.get(spawned[3].id).output = inspectionOutput
  await spawned[3].onTerminal({
    id: spawned[3].id,
    status: 'completed',
    output: inspectionOutput,
  })
  for (const resolve of terminalWaiters.get(spawned[3].id) || [])
    resolve({ timedOut: false, agent: records.get(spawned[3].id) })
  await waitFor(() => spawned.length === 6, 'the changed-input verification agents to restart')
  for (const [index, entry] of spawned.slice(4).entries()) {
    const output = `Changed verification ${index + 1} done.`
    records.get(entry.id).status = 'completed'
    records.get(entry.id).output = output
    await entry.onTerminal({ id: entry.id, status: 'completed', output })
    for (const resolve of terminalWaiters.get(entry.id) || [])
      resolve({ timedOut: false, agent: records.get(entry.id) })
  }
  const changed = await changedPromise
  assert.equal(changed.taskCount, 3)
  assert.equal(service.get('session-1').tasks.length, 3)
  assert.notEqual(
    service.findTask('session-1', 'inspect_inspect_1').workflowFingerprint,
    previousFingerprint,
  )

  const restored = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await restored.init()
  assert.equal(
    restored.get('session-1').tasks.every((task) => task.autoStart),
    true,
  )
})

test('dynamic Team workflow scripts cannot import modules or escape the workspace', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-script-guard-'))
  const workspace = join(directory, 'workspace')
  t.after(() => rm(directory, { recursive: true, force: true }))
  await mkdir(workspace)
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { objective: 'Validate workflow isolation.' })
  const multiAgents = {
    spawn: async () => null,
    list: () => [],
    find: () => null,
    sendMessageFromAgent: async () => null,
    sendMessage: async () => null,
    followup: async () => null,
    interrupt: () => null,
  }
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: workspace,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    getToolRisk: () => null,
    createInheritedCustomTools: () => [],
    waitAgent: async () => ({ timedOut: true, agent: null }),
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: () => {},
  })
  const validMeta =
    "export const meta = { name: 'guard-test', description: 'Validate workflow isolation' }\n"
  await writeFile(join(workspace, 'imports.js'), `${validMeta}await import('node:fs')\n`)
  await writeFile(
    join(workspace, 'escape.js'),
    `${validMeta}return agent.constructor('return process')()\n`,
  )
  await writeFile(
    join(workspace, 'meta-code.js'),
    "export const meta = { name: (() => 'unsafe')(), description: 'invalid' }\nreturn null\n",
  )
  await writeFile(
    join(workspace, 'async-loop.js'),
    `${validMeta}await Promise.resolve()\nfor (let value = 0; value >= 0; value += 1) {}\n`,
  )
  await writeFile(join(directory, 'outside.js'), `${validMeta}return 'outside'\n`)

  await assert.rejects(runtime.runScript('imports.js'), /forbidden capability/i)
  await assert.rejects(runtime.runScript('escape.js'), /code generation from strings disallowed/i)
  await assert.rejects(runtime.runScript('meta-code.js'), /meta values must be quoted strings/i)
  await assert.rejects(runtime.runScript('../outside.js'), /inside the current workspace/i)
  await assert.rejects(runtime.runScript('async-loop.js'), /did not yield|timed out/i)
  assert.equal(service.get('session-1').tasks.length, 0)
})

test('team adapter registers real spawn results and synchronizes terminal notifications', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-team-adapter-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const service = new TeamWorkflowService({ path: join(directory, 'teams.json') })
  await service.init()
  await service.ensure('session-1', { goalId: 'goal-1', objective: 'Coordinate two tracks.' })
  let onProgress
  const multiAgents = {
    async spawn(input) {
      onProgress = input.onProgress
      return {
        id: 'agent-1',
        canonicalName: '/root/inspect_1',
        status: 'completed',
        output: 'Done.',
      }
    },
    list: () => [],
    find: () => null,
    sendMessage: async () => null,
    followup: async () => null,
    interrupt: () => null,
  }
  const emittedUpdates = []
  const runtime = createMultiAgentRuntime({
    getRuntimeSession: () => ({
      sessionId: 'session-1',
      model: { provider: 'test', id: 'model' },
      thinkingLevel: 'medium',
      getActiveToolNames: () => ['read'],
    }),
    multiAgents,
    teamWorkflows: service,
    effectiveCwd: directory,
    executionMode: 'full-access',
    enabledTools: ['read'],
    planReader: null,
    baseToolNames: ['read'],
    getExecutionMode: () => 'full-access',
    getToolRisk: () => null,
    createInheritedCustomTools: () => [],
    waitAgent: async () => ({ timedOut: false, agents: [], agent: null }),
    installSubagentPermissions: () => {},
    onCompleted: () => {},
    emitAgentUpdate: (_sessionId, agent) => emittedUpdates.push(agent),
  })

  const result = await runtime.spawn({ taskName: 'inspect', message: 'Inspect the project.' })
  assert.equal(result.teamTaskId, service.get('session-1').tasks[0].id)
  assert.equal(service.get('session-1').tasks[0].status, 'completed')
  const updatesBeforeProgress = emittedUpdates.length
  await onProgress({ id: 'agent-1', status: 'completed', output: 'Updated.' })
  await new Promise((resolve) => setImmediate(resolve))
  assert.equal(service.get('session-1').tasks[0].output, 'Done.')
  assert.equal(emittedUpdates.length, updatesBeforeProgress)
  await service.write
})
