import assert from 'node:assert/strict'
import test from 'node:test'
import {
  hasConfiguredChatProvider,
  shouldShowModelOnboarding,
} from '../../src/features/config/model-onboarding.ts'

const provider = (overrides = {}) => ({
  configured: false,
  enabled: true,
  models: [{ kind: 'chat' }],
  ...overrides,
})

test('model onboarding only appears before a chat provider has been configured', () => {
  const empty = { providers: [provider()] }
  assert.equal(shouldShowModelOnboarding(empty, false), true)
  assert.equal(shouldShowModelOnboarding(empty, true), false)
  assert.equal(hasConfiguredChatProvider(empty), false)
  assert.equal(
    shouldShowModelOnboarding({ providers: [provider({ configured: true })] }, false),
    false,
  )
  const disabled = { providers: [provider({ configured: true, enabled: false })] }
  assert.equal(hasConfiguredChatProvider(disabled), true)
  assert.equal(shouldShowModelOnboarding(disabled, false), false)
  assert.equal(
    shouldShowModelOnboarding(
      { providers: [provider({ configured: true, models: [{ kind: 'image' }] })] },
      false,
    ),
    true,
  )
})
