import assert from 'node:assert/strict'
import test from 'node:test'
import { searchConfig } from '../../src/features/config/config-search.ts'

test('search targets request expansion of the group that owns the control', () => {
  const policy = searchConfig('运行策略', 'zh-CN').find(
    ({ entry }) => entry.id === 'models-runtime-policy',
  )?.entry
  assert.equal(policy?.card, 'models-advanced')
  assert.equal(policy?.targetId, 'models-runtime-policy')
  assert.equal(policy?.expandGroup, 'models-advanced')
  const visual = searchConfig('visual', 'en-US').find(
    ({ entry }) => entry.id === 'models-visual',
  )?.entry
  assert.equal(visual?.expandGroup, 'models-advanced')
  const font = searchConfig('font', 'en-US').find(
    ({ entry }) => entry.id === 'interface-style',
  )?.entry
  assert.equal(font?.expandGroup, 'interface-advanced')
})

test('search omits controls and sections unavailable on the current runtime', () => {
  const options = {
    capabilities: {
      features: { desktopPet: false, remoteAccess: false, visualGeneration: false },
    },
    mobileApp: true,
  }
  assert.deepEqual(searchConfig('pet', 'en-US', options), [])
  assert.deepEqual(searchConfig('visual', 'en-US', options), [])
  assert.ok(
    searchConfig('server', 'en-US', options).some(({ entry }) => entry.section === 'mobile-server'),
  )
  assert.ok(
    !searchConfig('remote', 'en-US', options).some(
      ({ entry }) => entry.section === 'remote-access',
    ),
  )
  assert.ok(
    !searchConfig('server', 'en-US', { mobileApp: false }).some(
      ({ entry }) => entry.section === 'mobile-server',
    ),
  )
})
