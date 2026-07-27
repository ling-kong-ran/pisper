import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { TaskListService } from '../services/task-list-service.mjs'
import { createTaskListTools } from '../tools/app/task-list.mjs'

test('task lists persist structured progress per primary session', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-task-list-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'task-lists.json')
  const service = new TaskListService({ path, now: () => Date.parse('2026-07-22T01:02:03.000Z') })
  await service.init()

  const updated = await service.replace('session-1', [
    { id: 'inspect', title: 'Inspect the implementation', status: 'completed' },
    { id: 'verify', title: 'Run focused tests', status: 'in_progress', note: 'Browser and asset coverage' },
  ])

  assert.deepEqual(updated.counts, { pending: 0, inProgress: 1, completed: 1, blocked: 0, total: 2 })
  assert.equal(updated.items[0].createdAt, '2026-07-22T01:02:03.000Z')
  const restored = new TaskListService({ path })
  await restored.init()
  assert.deepEqual(restored.get('session-1'), updated)
  assert.match(await readFile(path, 'utf8'), /"session-1"/)
})

test('task list tools return structured details and can clear the list', async () => {
  let current = { sessionId: 'session-1', items: [], counts: { total: 0 } }
  const tools = createTaskListTools({
    getTaskList: () => current,
    updateTaskList: (items) => {
      current = { sessionId: 'session-1', items, counts: { total: items.length } }
      return current
    },
  })

  const updateTool = tools.find((tool) => tool.name === 'update_task_list')
  assert.deepEqual(updateTool.parameters.properties.items.items.properties.status.enum, [
    'pending',
    'in_progress',
    'completed',
    'blocked',
  ])
  const updated = await updateTool.execute('call-1', {
    items: [{ id: 'one', title: 'One task', status: 'pending' }],
  })
  const read = await tools.find((tool) => tool.name === 'get_task_list').execute('call-2', {})

  assert.equal(updated.details.taskList.items.length, 1)
  assert.deepEqual(read.details.taskList, current)
})

test('task items persist assignee and dependsOn with defaults and validation', async (t) => {
  const directory = await mkdtemp(join(tmpdir(), 'pisper-task-list-'))
  t.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'task-lists.json')
  const service = new TaskListService({ path })
  await service.init()

  const updated = await service.replace('session-1', [
    { id: 'research', title: 'Research the module', status: 'completed', assignee: '/root/researcher' },
    { id: 'implement', title: 'Implement the change', status: 'pending', assignee: '/root/builder', dependsOn: ['research'] },
    { id: 'docs', title: 'Update docs', status: 'pending' },
  ])

  assert.equal(updated.items[0].assignee, '/root/researcher')
  assert.deepEqual(updated.items[1].dependsOn, ['research'])
  // Missing fields default to empty so older lists stay readable.
  assert.equal(updated.items[2].assignee, '')
  assert.deepEqual(updated.items[2].dependsOn, [])

  const restored = new TaskListService({ path })
  await restored.init()
  assert.deepEqual(restored.get('session-1').items[1].dependsOn, ['research'])

  // Duplicate dependency ids collapse and dependency-blocked work is counted consistently.
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
    () => service.replace('session-1', [{ id: 'x', title: 'X', status: 'pending', dependsOn: ['not a valid id!'] }]),
    /Invalid dependency task id/,
  )
  await assert.rejects(
    () => service.replace('session-1', [{ id: 'x', title: 'X', status: 'pending', dependsOn: ['missing'] }]),
    /Unknown dependency task id: missing/,
  )
  await assert.rejects(
    () => service.replace('session-1', [{ id: 'x', title: 'X', status: 'pending', dependsOn: ['x'] }]),
    /Task cannot depend on itself: x/,
  )
  await assert.rejects(
    () => service.replace('session-1', [
      { id: 'x', title: 'X', status: 'pending', dependsOn: ['y'] },
      { id: 'y', title: 'Y', status: 'pending', dependsOn: ['x'] },
    ]),
    /Task dependency cycle: x -> y -> x/,
  )
})
