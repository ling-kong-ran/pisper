import assert from 'node:assert/strict'
import test from 'node:test'
import { getActivityNavigation } from '../../src/app/activity-navigation.ts'
import { LEGACY_RUNTIME_CAPABILITIES } from '../../src/types/runtime-capabilities.ts'

// 旧功能栏分组只剩休眠的 WorkbenchNavigation 使用（外壳已改用 more-navigation，见
// more-navigation.test.mjs）。模块保留到第 6 期统一删除，这里的守卫随模块一起删除。

test('activity menus keep workspace, resources, automation and extensions separately reachable', () => {
  const groups = getActivityNavigation((key) => key, LEGACY_RUNTIME_CAPABILITIES)
  assert.deepEqual(
    groups.map((group) => group.id),
    ['workspace', 'resources', 'automation', 'extensions'],
  )
  const ids = groups.flatMap((group) => group.items.map((item) => item.id))
  assert.equal(ids.length, new Set(ids).size)
  for (const id of ['chat', 'assets', 'workflows', 'schedules', 'skills', 'mcp', 'plugins']) {
    assert.ok(ids.includes(id), `missing ${id}`)
  }
})

test('capability filtering does not expose unavailable extension routes', () => {
  const capabilities = {
    ...LEGACY_RUNTIME_CAPABILITIES,
    features: { ...LEGACY_RUNTIME_CAPABILITIES.features, skills: false, workflows: false },
  }
  const ids = getActivityNavigation((key) => key, capabilities).flatMap((group) =>
    group.items.map((item) => item.id),
  )
  assert.ok(!ids.includes('skills'))
  assert.ok(!ids.includes('workflows'))
  assert.ok(ids.includes('chat'))
})
