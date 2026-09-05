import assert from 'node:assert/strict'
import test from 'node:test'
import {
  importMobileWorkspaceDirectory,
  mobileWorkspaceMode,
} from '../../src/lib/mobile-workspace.ts'

function mobileFixture(
  t,
  {
    platform = 'android',
    mode = 'local',
    running = true,
    result = { path: '/workspace/import-1/Project' },
    failure,
  } = {},
) {
  const previous = globalThis.window
  const calls = []
  const invoke = async (command) => {
    calls.push(command)
    if (command === 'mobile_state') return { mode, onDevice: { running } }
    if (command === 'mobile_import_workspace_directory') {
      if (failure) throw new Error(failure)
      return result
    }
    throw new Error(`Unexpected native command: ${command}`)
  }
  globalThis.window = {
    __PISPER_MOBILE_APP__: true,
    __PISPER_MOBILE_PLATFORM__: platform,
    __TAURI__: { core: { invoke } },
  }
  t.after(() => {
    if (previous === undefined) delete globalThis.window
    else globalThis.window = previous
  })
  return calls
}

test('Android local picker returns a runtime path and never sends a destination from the browser', async (t) => {
  const calls = mobileFixture(t)
  assert.equal(await mobileWorkspaceMode(), 'local')
  assert.equal(await importMobileWorkspaceDirectory(), '/workspace/import-1/Project')
  assert.deepEqual(calls, ['mobile_state', 'mobile_state', 'mobile_import_workspace_directory'])
})

test('native picker cancellation is not an empty directory or an error', async (t) => {
  mobileFixture(t, { result: { path: null } })
  assert.equal(await importMobileWorkspaceDirectory(), null)
})

test('remote mode rejects phone directory import before opening native UI', async (t) => {
  const calls = mobileFixture(t, { mode: 'remote' })
  assert.equal(await mobileWorkspaceMode(), 'remote')
  await assert.rejects(importMobileWorkspaceDirectory(), /requires_local_runtime/)
  assert.deepEqual(calls, ['mobile_state', 'mobile_state'])
})

test('stopped local Runtime cannot receive an imported directory', async (t) => {
  const calls = mobileFixture(t, { running: false })
  await assert.rejects(importMobileWorkspaceDirectory(), /requires_local_runtime/)
  assert.deepEqual(calls, ['mobile_state'])
})

test('unsupported platforms never invoke Android folder selection', async (t) => {
  const calls = mobileFixture(t, { platform: 'ios' })
  await assert.rejects(importMobileWorkspaceDirectory(), /unsupported/)
  assert.deepEqual(calls, [])
})

test('native errors propagate without returning the previous working directory', async (t) => {
  mobileFixture(t, { failure: 'workspace_import_limit_exceeded' })
  await assert.rejects(importMobileWorkspaceDirectory(), /workspace_import_limit_exceeded/)
})

test('SAF URIs, missing paths and relative paths cannot become a Node working directory', async (t) => {
  mobileFixture(t)
  for (const path of ['content://provider/tree/project', '', 'Project', undefined, 123]) {
    window.__TAURI__.core.invoke = async (command) =>
      command === 'mobile_state' ? { mode: 'local', onDevice: { running: true } } : { path }
    await assert.rejects(importMobileWorkspaceDirectory(), /invalid_path/)
  }
})

test('native bridge fallback works while web mode never asks for mobile state', async (t) => {
  const calls = mobileFixture(t)
  window.__TAURI_INTERNALS__ = window.__TAURI__.core
  delete window.__TAURI__
  assert.equal(await importMobileWorkspaceDirectory(), '/workspace/import-1/Project')
  calls.length = 0
  window.__PISPER_MOBILE_APP__ = false
  assert.equal(await mobileWorkspaceMode(), null)
  assert.deepEqual(calls, [])
})
