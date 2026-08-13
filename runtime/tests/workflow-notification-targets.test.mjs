import assert from 'node:assert/strict'
import test from 'node:test'
import {
  AgentRuntimeFacade,
  enabledNotificationTargets,
  filterWorkflowNotificationTargets,
} from '../runtime/agent-runtime-facade.mjs'

test('workflow notifications retain only individually enabled targets', () => {
  const enabledTargets = enabledNotificationTargets({
    browser: { enabled: false },
    connections: {
      feishu: { enabled: false },
      weixin: { enabled: true },
    },
  })
  const input = {
    notifications: ['browser', 'feishu', 'weixin'],
    nodes: [
      {
        id: 'notify',
        kind: 'notification',
        notificationTargets: ['browser', 'feishu', 'weixin'],
      },
      { id: 'prompt', kind: 'prompt' },
    ],
  }

  assert.deepEqual([...enabledTargets], ['weixin'])
  assert.deepEqual(filterWorkflowNotificationTargets(input, enabledTargets), {
    notifications: ['weixin'],
    nodes: [
      { id: 'notify', kind: 'notification', notificationTargets: ['weixin'] },
      { id: 'prompt', kind: 'prompt' },
    ],
  })
  assert.deepEqual(input.notifications, ['browser', 'feishu', 'weixin'])
})

test('workflow mutation APIs filter disabled targets before persistence', async () => {
  const created = []
  const updated = []
  const runtime = Object.assign(Object.create(AgentRuntimeFacade.prototype), {
    notificationSettings: {
      getState: async () => ({
        browser: { enabled: false },
        connections: { feishu: { enabled: true }, weixin: { enabled: false } },
      }),
    },
    workflows: {
      create: async (input) => {
        created.push(input)
        return { id: 'workflow-1' }
      },
      update: async (id, input) => {
        updated.push([id, input])
        return { id }
      },
    },
    getWorkflows: async () => ({ workflows: [] }),
  })
  const input = {
    notifications: ['browser', 'feishu', 'weixin'],
    nodes: [{ id: 'notify', notificationTargets: ['browser', 'feishu', 'weixin'] }],
  }

  await runtime.createWorkflow(input)
  await runtime.updateWorkflow('workflow-1', input)

  assert.deepEqual(created[0].notifications, ['feishu'])
  assert.deepEqual(created[0].nodes[0].notificationTargets, ['feishu'])
  assert.deepEqual(updated[0][1].notifications, ['feishu'])
  assert.deepEqual(updated[0][1].nodes[0].notificationTargets, ['feishu'])
})
