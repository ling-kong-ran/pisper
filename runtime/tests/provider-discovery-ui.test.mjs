import assert from 'node:assert/strict'
import test from 'node:test'
import { providerDiscoveryShouldRender } from '../../src/features/config/provider-discovery-state.ts'

function discovery(providers = [], errors = []) {
  return { providers, errors }
}

test('provider discovery hides only settled configurations without actionable results', () => {
  assert.equal(providerDiscoveryShouldRender(discovery(), false, ''), false)
  assert.equal(
    providerDiscoveryShouldRender(discovery([{ imported: true, importable: true }]), false, ''),
    false,
  )
  assert.equal(
    providerDiscoveryShouldRender(
      discovery([
        { imported: false, importable: false, warnings: [{ code: 'login_auth_not_imported' }] },
      ]),
      false,
      '',
    ),
    true,
  )
})

test('provider discovery remains visible for loading, importable, error, and conflict states', () => {
  assert.equal(providerDiscoveryShouldRender(discovery(), true, ''), true)
  assert.equal(
    providerDiscoveryShouldRender(discovery([{ importable: true, imported: false }]), false, ''),
    true,
  )
  assert.equal(providerDiscoveryShouldRender(discovery(), false, 'scan failed'), true)
  assert.equal(
    providerDiscoveryShouldRender(discovery([], [{ code: 'invalid_toml' }]), false, ''),
    true,
  )
  assert.equal(
    providerDiscoveryShouldRender(
      discovery([{ importable: false, imported: false, conflict: true }]),
      false,
      '',
    ),
    true,
  )
})
