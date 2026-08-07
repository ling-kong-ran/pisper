import assert from 'node:assert/strict'
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { PlanService } from '../services/plan-service.mjs'
import { TaskListService } from '../services/task-list-service.mjs'
import {
  createPlanTools,
  PLAN_COMPATIBILITY_TOOL_NAMES,
  PLAN_TOOL_NAMES,
} from '../tools/app/plan.mjs'
import { createTaskListTools } from '../tools/app/task-list.mjs'

function storedPlan(items, updatedAt = '2026-07-22T01:02:03.000Z') {
  return { items, updatedAt }
}

test('plans persist structured progress per primary session in the canonical store shape', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'pisper-plans.json')
  const service = new PlanService({ path, now: () => Date.parse('2026-07-22T01:02:03.000Z') })
  await service.init()

  const updated = await service.replace('session-1', [
    { id: 'inspect', title: 'Inspect the implementation', status: 'completed' },
    { id: 'verify', title: 'Run focused tests', status: 'in_progress', note: 'Protocol coverage' },
  ])

  assert.deepEqual(updated.counts, {
    pending: 0,
    inProgress: 1,
    completed: 1,
    blocked: 0,
    total: 2,
  })
  assert.equal(updated.items[0].createdAt, '2026-07-22T01:02:03.000Z')
  const restored = new PlanService({ path })
  await restored.init()
  assert.deepEqual(restored.get('session-1'), updated)
  const stored = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(Object.hasOwn(stored, 'lists'), false)
  assert.equal(stored.plans['session-1'].items.length, 2)
})

test('queued plan updates commit in order without exposing an unpersisted snapshot', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-queue-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'pisper-plans.json')
  const service = new PlanService({ path, now: () => Date.parse('2026-07-22T01:02:03.000Z') })
  await service.init()

  const firstWrite = service.replace('session-1', [
    { id: 'one', title: 'First snapshot', status: 'in_progress' },
  ])
  const secondWrite = service.replace('session-1', [
    { id: 'one', title: 'Final snapshot', status: 'completed' },
  ])
  const [first, second] = await Promise.all([firstWrite, secondWrite])

  assert.equal(first.items[0].title, 'First snapshot')
  assert.equal(second.items[0].title, 'Final snapshot')
  assert.equal(second.items[0].createdAt, first.items[0].createdAt)
  const restored = new PlanService({ path })
  await restored.init()
  assert.deepEqual(restored.get('session-1'), second)
})

test('canonical plan tools return { plan }, aliases share behavior, and empty items clear the plan', async () => {
  let current = { sessionId: 'session-1', items: [], counts: { total: 0 }, updatedAt: null }
  const tools = createPlanTools({
    getPlan: () => current,
    updatePlan: (items) => {
      current = { sessionId: 'session-1', items, counts: { total: items.length }, updatedAt: null }
      return current
    },
  })

  assert.deepEqual(
    tools.slice(0, 2).map((tool) => tool.name),
    PLAN_TOOL_NAMES,
  )
  assert.deepEqual(
    tools.slice(2).map((tool) => tool.name),
    PLAN_COMPATIBILITY_TOOL_NAMES,
  )
  const updateTool = tools.find((tool) => tool.name === 'update_plan')
  assert.deepEqual(updateTool.parameters.properties.items.items.properties.status.enum, [
    'pending',
    'in_progress',
    'completed',
    'blocked',
  ])
  const updated = await updateTool.execute('call-1', {
    items: [{ id: 'one', title: 'One plan item', status: 'pending' }],
  })
  const read = await tools.find((tool) => tool.name === 'get_plan').execute('call-2', {})
  const legacyRead = await tools.find((tool) => tool.name === 'get_task_list').execute('call-3', {})
  const cleared = await updateTool.execute('call-4', { items: [] })

  assert.equal(updated.details.plan.items.length, 1)
  assert.deepEqual(read.details.plan.items, [
    { id: 'one', title: 'One plan item', status: 'pending' },
  ])
  assert.deepEqual(legacyRead.details.plan, read.details.plan)
  assert.equal(Object.hasOwn(updated.details, 'taskList'), false)
  assert.equal(cleared.details.plan.items.length, 0)
  assert.equal(tools.find((tool) => tool.name === 'get_task_list').promptSnippet, undefined)
  assert.equal(tools.find((tool) => tool.name === 'update_task_list').promptGuidelines, undefined)
})

test('plan items persist assignee and dependsOn with defaults and validation', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'pisper-plans.json')
  const service = new PlanService({ path })
  await service.init()

  const updated = await service.replace('session-1', [
    {
      id: 'research',
      title: 'Research the module',
      status: 'completed',
      assignee: '/root/researcher',
    },
    {
      id: 'implement',
      title: 'Implement the change',
      status: 'pending',
      assignee: '/root/builder',
      dependsOn: ['research'],
    },
    { id: 'docs', title: 'Update docs', status: 'pending' },
  ])

  assert.equal(updated.items[0].assignee, '/root/researcher')
  assert.deepEqual(updated.items[1].dependsOn, ['research'])
  assert.equal(updated.items[2].assignee, '')
  assert.deepEqual(updated.items[2].dependsOn, [])

  const restored = new PlanService({ path })
  await restored.init()
  assert.deepEqual(restored.get('session-1').items[1].dependsOn, ['research'])

  const deduped = await service.replace('session-1', [
    { id: 'b', title: 'B', status: 'completed' },
    { id: 'c', title: 'C', status: 'pending' },
    { id: 'a', title: 'A', status: 'in_progress', dependsOn: ['b', 'b', 'c'] },
    { id: 'manual-block', title: 'Manual blocker', status: 'blocked' },
  ])
  assert.deepEqual(deduped.items[2].dependsOn, ['b', 'c'])
  assert.deepEqual(deduped.counts, {
    pending: 1,
    inProgress: 0,
    completed: 1,
    blocked: 2,
    total: 4,
  })

  await assert.rejects(
    () =>
      service.replace('session-1', [
        { id: 'x', title: 'X', status: 'pending', dependsOn: ['not a valid id!'] },
      ]),
    /Invalid dependency plan item id/,
  )
  await assert.rejects(
    () =>
      service.replace('session-1', [
        { id: 'x', title: 'X', status: 'pending', dependsOn: ['missing'] },
      ]),
    /Unknown dependency plan item id: missing/,
  )
  await assert.rejects(
    () =>
      service.replace('session-1', [{ id: 'x', title: 'X', status: 'pending', dependsOn: ['x'] }]),
    /Plan item cannot depend on itself: x/,
  )
  await assert.rejects(
    () =>
      service.replace('session-1', [
        { id: 'x', title: 'X', status: 'pending', dependsOn: ['y'] },
        { id: 'y', title: 'Y', status: 'pending', dependsOn: ['x'] },
      ]),
    /Plan dependency cycle: x -> y -> x/,
  )
})

test('first startup backs up and atomically migrates a valid legacy task-list store', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-migration-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const legacyPath = join(directory, 'pisper-task-lists.json')
  const path = join(directory, 'pisper-plans.json')
  const legacy = JSON.stringify(
    {
      version: 1,
      lists: {
        'session-legacy': storedPlan([
          { id: 'valid', title: 'Migrate this item', status: 'in_progress' },
          { id: 'invalid', title: '', status: 'pending' },
        ]),
      },
    },
    null,
    2,
  )
  await writeFile(legacyPath, `${legacy}\n`, 'utf8')
  await writeFile(`${legacyPath}.bak`, 'stale backup\n', 'utf8')

  const service = new PlanService({ path, legacyPath })
  await service.init()

  assert.equal(service.readingLegacy, false)
  assert.equal(service.get('session-legacy').items.length, 1)
  assert.equal(await readFile(legacyPath, 'utf8'), `${legacy}\n`)
  assert.equal(await readFile(`${legacyPath}.bak`, 'utf8'), `${legacy}\n`)
  const migrated = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(Object.hasOwn(migrated, 'lists'), false)
  assert.equal(migrated.plans['session-legacy'].items[0].title, 'Migrate this item')
  assert.deepEqual((await readdir(directory)).sort(), [
    'pisper-plans.json',
    'pisper-task-lists.json',
    'pisper-task-lists.json.bak',
  ])
})

test('backup failure blocks canonical writes until the legacy source is safely backed up', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-backup-failure-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const legacyPath = join(directory, 'pisper-task-lists.json')
  const backupPath = `${legacyPath}.bak`
  const path = join(directory, 'pisper-plans.json')
  const legacy = `${JSON.stringify(
    {
      version: 1,
      lists: {
        'session-legacy': storedPlan([
          { id: 'keep', title: 'Keep legacy data', status: 'pending' },
        ]),
      },
    },
    null,
    2,
  )}\n`
  await writeFile(legacyPath, legacy, 'utf8')
  await mkdir(backupPath)

  const service = new PlanService({ path, legacyPath })
  await service.init()

  assert.equal(service.readingLegacy, true)
  assert.equal(service.get('session-legacy').items[0].title, 'Keep legacy data')
  await assert.rejects(
    () =>
      service.replace('session-legacy', [
        { id: 'next', title: 'Write after backup', status: 'in_progress' },
      ]),
    /backup path is not a file/,
  )
  await assert.rejects(() => readFile(path, 'utf8'))
  assert.equal(await readFile(legacyPath, 'utf8'), legacy)
  assert.equal(service.get('session-legacy').items[0].title, 'Keep legacy data')

  await rm(backupPath, { recursive: true, force: true })
  const updated = await service.replace('session-legacy', [
    { id: 'next', title: 'Write after backup', status: 'completed' },
  ])

  assert.equal(service.readingLegacy, false)
  assert.equal(updated.items[0].status, 'completed')
  assert.equal(await readFile(backupPath, 'utf8'), legacy)
  assert.equal(await readFile(legacyPath, 'utf8'), legacy)
  const stored = JSON.parse(await readFile(path, 'utf8'))
  assert.equal(Object.hasOwn(stored, 'lists'), false)
  assert.equal(stored.plans['session-legacy'].items[0].title, 'Write after backup')
  assert.equal(
    (await readdir(directory)).some((name) => name.endsWith('.tmp')),
    false,
  )
})

test('migration write failure continues serving legacy data without clearing its source or backup', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-migration-failure-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const legacyPath = join(directory, 'pisper-task-lists.json')
  const blockedParent = join(directory, 'not-a-directory')
  const path = join(blockedParent, 'pisper-plans.json')
  const legacy = `${JSON.stringify(
    {
      version: 1,
      lists: {
        'session-legacy': storedPlan([
          { id: 'keep', title: 'Keep legacy data', status: 'pending' },
        ]),
      },
    },
    null,
    2,
  )}\n`
  await writeFile(legacyPath, legacy, 'utf8')
  await writeFile(blockedParent, 'blocks canonical store creation', 'utf8')

  const service = new PlanService({ path, legacyPath })
  await service.init()

  assert.equal(service.readingLegacy, true)
  assert.equal(service.get('session-legacy').items[0].title, 'Keep legacy data')
  assert.equal(await readFile(legacyPath, 'utf8'), legacy)
  assert.equal(await readFile(`${legacyPath}.bak`, 'utf8'), legacy)
  await assert.rejects(() => readFile(path, 'utf8'))
})

test('an existing canonical store takes precedence over the legacy migration source', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-canonical-precedence-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const legacyPath = join(directory, 'pisper-task-lists.json')
  const path = join(directory, 'pisper-plans.json')
  await writeFile(
    legacyPath,
    JSON.stringify({
      lists: { session: storedPlan([{ id: 'old', title: 'Legacy item', status: 'pending' }]) },
    }),
    'utf8',
  )
  await writeFile(
    path,
    JSON.stringify({
      plans: {
        session: storedPlan([{ id: 'new', title: 'Canonical item', status: 'in_progress' }]),
      },
    }),
    'utf8',
  )

  const service = new PlanService({ path, legacyPath })
  await service.init()

  assert.equal(service.get('session').items[0].title, 'Canonical item')
  assert.equal((await readdir(directory)).includes('pisper-task-lists.json.bak'), false)
})

test('old and new persisted root fields normalize, while every subsequent write uses { plans }', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-plan-normalize-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const oldPath = join(directory, 'old-root.json')
  const newPath = join(directory, 'new-root.json')
  const item = { id: 'one', title: 'Normalized item', status: 'pending' }
  await writeFile(oldPath, JSON.stringify({ lists: { old: storedPlan([item]) } }), 'utf8')
  await writeFile(newPath, JSON.stringify({ plans: { next: storedPlan([item]) } }), 'utf8')

  const oldService = new PlanService({ path: oldPath })
  const newService = new PlanService({ path: newPath })
  await oldService.init()
  await newService.init()
  assert.equal(oldService.get('old').items[0].title, 'Normalized item')
  assert.equal(newService.get('next').items[0].title, 'Normalized item')

  await oldService.replace('old', [])
  const rewritten = JSON.parse(await readFile(oldPath, 'utf8'))
  assert.deepEqual(rewritten, { version: 1, plans: {} })
  const restored = new PlanService({ path: oldPath })
  await restored.init()
  assert.deepEqual(restored.get('old'), {
    sessionId: 'old',
    items: [],
    counts: { pending: 0, inProgress: 0, completed: 0, blocked: 0, total: 0 },
    updatedAt: null,
  })
})

test('one-release task-list modules remain thin compatibility wrappers', async () => {
  assert.equal(TaskListService, PlanService)
  const legacyTools = createTaskListTools({
    getTaskList: () => ({ sessionId: 'legacy', items: [], counts: { total: 0 }, updatedAt: null }),
    updateTaskList: () => ({
      sessionId: 'legacy',
      items: [],
      counts: { total: 0 },
      updatedAt: null,
    }),
  })
  assert.deepEqual(
    legacyTools.map((tool) => tool.name),
    PLAN_COMPATIBILITY_TOOL_NAMES,
  )
  const result = await legacyTools[0].execute('legacy-read', {})
  assert.equal(result.details.plan.sessionId, 'legacy')
})
