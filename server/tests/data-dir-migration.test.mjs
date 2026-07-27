import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { resolveAgentDataDir } from '../data-dir-migration.mjs'

function withTempHome(t) {
  const home = mkdtempSync(join(tmpdir(), 'pisper-migrate-'))
  t.after(() => rmSync(home, { recursive: true, force: true }))
  return home
}

function seedLegacy(home, marker = 'legacy-session') {
  const legacyAgent = join(home, '.vesper', 'agent')
  mkdirSync(legacyAgent, { recursive: true })
  writeFileSync(join(legacyAgent, 'marker.txt'), marker)
  return legacyAgent
}

test('explicit PISPER_AGENT_DIR wins and never triggers migration', (t) => {
  const home = withTempHome(t)
  seedLegacy(home)
  const custom = join(home, 'custom-dir')
  const dir = resolveAgentDataDir({ env: { PISPER_AGENT_DIR: custom }, home, log: () => {} })
  assert.equal(dir, custom)
  assert.ok(existsSync(join(home, '.vesper')), 'legacy dir left untouched')
  assert.ok(!existsSync(join(home, '.pisper')), 'no migration performed')
})

test('legacy VESPER_AGENT_DIR is honored for backward compatibility', (t) => {
  const home = withTempHome(t)
  const custom = join(home, 'old-custom')
  const dir = resolveAgentDataDir({ env: { VESPER_AGENT_DIR: custom }, home, log: () => {} })
  assert.equal(dir, custom)
})

test('migrates ~/.vesper to ~/.pisper when only legacy data exists', (t) => {
  const home = withTempHome(t)
  seedLegacy(home)
  const dir = resolveAgentDataDir({ env: {}, home, log: () => {} })
  assert.equal(dir, join(home, '.pisper', 'agent'))
  assert.equal(readFileSync(join(dir, 'marker.txt'), 'utf8'), 'legacy-session')
  assert.ok(!existsSync(join(home, '.vesper')), 'legacy dir moved away')
})

test('keeps existing ~/.pisper and leaves ~/.vesper as backup', (t) => {
  const home = withTempHome(t)
  seedLegacy(home, 'old-data')
  const currentAgent = join(home, '.pisper', 'agent')
  mkdirSync(currentAgent, { recursive: true })
  writeFileSync(join(currentAgent, 'marker.txt'), 'new-data')
  const dir = resolveAgentDataDir({ env: {}, home, log: () => {} })
  assert.equal(dir, currentAgent)
  assert.equal(readFileSync(join(dir, 'marker.txt'), 'utf8'), 'new-data', 'new data wins')
  assert.equal(readFileSync(join(home, '.vesper', 'agent', 'marker.txt'), 'utf8'), 'old-data', 'legacy kept as backup')
})

test('uses ~/.pisper/agent by default when no data exists yet', (t) => {
  const home = withTempHome(t)
  const dir = resolveAgentDataDir({ env: {}, home, log: () => {} })
  assert.equal(dir, join(home, '.pisper', 'agent'))
})
