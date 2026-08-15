import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  providerDiscoveryShouldCollapse,
  providerDiscoveryShouldRender,
} from '../../src/features/config/provider-discovery-state.ts'

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

test('provider discovery collapses after scanning when no provider can be imported', () => {
  assert.equal(providerDiscoveryShouldCollapse(discovery(), true), false)
  assert.equal(providerDiscoveryShouldCollapse(discovery(), false), true)
  assert.equal(
    providerDiscoveryShouldCollapse(
      discovery([{ importable: true, imported: false, conflict: false }]),
      false,
    ),
    false,
  )
  assert.equal(
    providerDiscoveryShouldCollapse(
      discovery([{ importable: true, imported: false, conflict: true }]),
      false,
    ),
    true,
  )
  assert.equal(
    providerDiscoveryShouldCollapse(discovery([{ importable: true, imported: true }]), false),
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

test('provider import errors render in the discovery panel instead of the Provider detail form', async () => {
  const modelsSettings = await readFile('src/features/config/ModelsSettings.tsx', 'utf8')
  assert.match(
    modelsSettings,
    /<ProviderDiscovery[\s\S]*?error=\{discovery\.error \|\| discovery\.operationError\}/,
  )
  assert.match(modelsSettings, /<ProviderSettingsActions[\s\S]*?error=\{settings\.error\}/)
  assert.doesNotMatch(modelsSettings, /error=\{settings\.error \|\| discovery\.operationError\}/)
})
