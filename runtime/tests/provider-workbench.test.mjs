import assert from 'node:assert/strict'
import test from 'node:test'
import { getProviderWorkbenchState } from '../../src/features/config/model/provider-workbench-state.ts'
import { getSettingsNavigation } from '../../src/app/routes/settings-navigation.ts'

const provider = (id, overrides = {}) => ({
  id,
  name: id,
  type: 'chat',
  api: 'openai-completions',
  models: [],
  configured: false,
  enabled: true,
  ...overrides,
})

test('provider workbench hides unused presets while retaining saved and disabled connections', () => {
  const config = {
    provider: 'unused-preset',
    providers: [
      provider('unused-preset'),
      provider('configured-builtin', { configured: true }),
      provider('disabled-builtin', { configured: true, enabled: false }),
      provider('incomplete-custom', { custom: true, enabled: false }),
      provider('visual-custom', { custom: true, configured: true, type: 'visual' }),
    ],
  }
  const before = structuredClone(config)
  const { providers, selected } = getProviderWorkbenchState(config, 'unused-preset')
  assert.deepEqual(
    providers.map((item) => item.id),
    ['configured-builtin', 'disabled-builtin', 'incomplete-custom', 'visual-custom'],
  )
  assert.equal(selected.id, 'configured-builtin')
  assert.deepEqual(config, before, 'filtering the view must not delete saved configuration')
})

test('provider selection survives switching and falls back after removal', () => {
  const config = {
    provider: 'first',
    defaultProvider: 'default',
    providers: [
      provider('first', { configured: true }),
      provider('default', { configured: true }),
      provider('new-connection', { custom: true }),
    ],
  }
  assert.equal(getProviderWorkbenchState(config, 'new-connection').selected.id, 'new-connection')
  assert.equal(getProviderWorkbenchState(config, 'deleted').selected.id, 'default')
  assert.equal(getProviderWorkbenchState(config, '').selected.id, 'default')
  assert.equal(
    getProviderWorkbenchState({ ...config, defaultProvider: 'deleted' }, '').selected.id,
    'first',
  )
})

test('a fresh provider configuration renders an empty workbench without selecting a preset', () => {
  for (const providers of [[], [provider('unused-preset')]]) {
    const result = getProviderWorkbenchState({ providers, provider: 'unused-preset' }, '')
    assert.deepEqual(result.providers, [])
    assert.equal(result.selected, undefined)
  }
})

test('unified settings includes models and appearance alongside platform-specific settings', () => {
  for (const mobileApp of [false, true]) {
    const items = getSettingsNavigation((key) => key, { mobileApp }).flatMap((group) => group.items)
    for (const id of ['models', 'interface', 'about', 'notifications']) {
      assert.deepEqual(items.find((item) => item.key === `config:${id}`)?.destination, {
        type: 'config',
        id,
      })
    }
    assert.equal(
      items.some((item) => item.key === 'config:mobile-server'),
      mobileApp,
    )
    assert.equal(
      items.some((item) => item.key === 'config:remote-access'),
      !mobileApp,
    )
    assert.equal(new Set(items.map((item) => item.key)).size, items.length)
  }
})
