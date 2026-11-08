// 会话页的协作提示（/live 的 workspace 字段）：按不可信输入校验，旧 Runtime 不带时什么都不显示。
import assert from 'node:assert/strict'
import test from 'node:test'

const { workspaceActivity } = await import('../../src/features/chat/session-workspace-activity.ts')

test('workspace activity keeps valid neighbours and messages and drops the rest', () => {
  assert.equal(workspaceActivity(undefined), null)
  assert.equal(workspaceActivity({ neighbours: [], messages: [] }), null)
  assert.equal(workspaceActivity([]), null)
  const activity = workspaceActivity({
    neighbours: [
      {
        id: 's1',
        title: 'Fix login',
        running: true,
        fileCount: 2,
        files: ['a.ts', 3],
        lastChangedAt: '2026-11-08T00:00:00.000Z',
      },
      { title: 'no id' },
    ],
    messages: [
      { id: 'm1', from: { id: 's1', title: 'Fix login' }, text: 'hold on', at: 'x' },
      { id: 'm2', from: { title: 'no sender id' }, text: 'dropped' },
    ],
  })
  assert.deepEqual(activity, {
    neighbours: [
      {
        id: 's1',
        title: 'Fix login',
        running: true,
        fileCount: 2,
        files: ['a.ts'],
        lastChangedAt: '2026-11-08T00:00:00.000Z',
      },
    ],
    messages: [{ id: 'm1', from: { id: 's1', title: 'Fix login' }, text: 'hold on', at: 'x' }],
  })
})
