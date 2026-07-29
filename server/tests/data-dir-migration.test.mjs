import assert from 'node:assert/strict'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import test from 'node:test'
import { resolveAgentDataDir } from '../data-dir-migration.mjs'

function withTempHome(t) {
  const home = mkdtempSync(join(tmpdir(), 'pisper-data-dir-'))
  t.after(() => rmSync(home, { recursive: true, force: true }))
  return home
}

test('explicit PISPER_AGENT_DIR wins', (t) => {
  const home = withTempHome(t)
  const custom = join(home, 'custom-dir')
  assert.equal(resolveAgentDataDir({ env: { PISPER_AGENT_DIR: custom }, home }), resolve(custom))
})

test('legacy environment variables cannot redirect Pisper to a Vesper data directory', (t) => {
  const home = withTempHome(t)
  const legacyCustom = join(home, 'legacy-custom')
  assert.equal(
    resolveAgentDataDir({ env: { VESPER_AGENT_DIR: legacyCustom }, home }),
    join(home, '.pisper', 'agent'),
  )
})

test('legacy home data is ignored instead of read or migrated', (t) => {
  const home = withTempHome(t)
  const legacyAgent = join(home, '.vesper', 'agent')
  mkdirSync(legacyAgent, { recursive: true })
  writeFileSync(join(legacyAgent, 'marker.txt'), 'legacy-data')

  assert.equal(resolveAgentDataDir({ env: {}, home }), join(home, '.pisper', 'agent'))
  assert.ok(existsSync(join(legacyAgent, 'marker.txt')), 'legacy data remains untouched')
  assert.ok(!existsSync(join(home, '.pisper')), 'path resolution performs no migration')
})

test('uses ~/.pisper/agent by default', (t) => {
  const home = withTempHome(t)
  assert.equal(resolveAgentDataDir({ env: {}, home }), join(home, '.pisper', 'agent'))
})
