import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  DEFAULT_EXECUTION_MODE,
  filterToolsForExecutionMode,
  migrateLegacyExecutionMode,
  normalizeExecutionMode,
  permissionModeForExecutionMode,
} from '../security/execution-mode.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'

test('execution modes normalize and migrate legacy permission settings', () => {
  assert.equal(normalizeExecutionMode('read-only'), 'read-only')
  assert.equal(normalizeExecutionMode('full-access'), 'full-access')
  assert.equal(normalizeExecutionMode('workspace'), 'full-access')
  assert.equal(normalizeExecutionMode('unknown'), DEFAULT_EXECUTION_MODE)
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'ignore' }), 'full-access')
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'ask' }), 'full-access')
  assert.equal(migrateLegacyExecutionMode({ executionMode: 'workspace' }), 'full-access')
  assert.equal(
    migrateLegacyExecutionMode({ executionMode: 'read-only', permissionMode: 'ignore' }),
    'read-only',
  )
  assert.equal(permissionModeForExecutionMode('read-only'), 'ask')
  assert.equal(permissionModeForExecutionMode('full-access'), 'ignore')
})

test('read-only execution exposes only low-risk analysis tools', () => {
  const names = [
    'read',
    'grep',
    'edit',
    'bash',
    'discover_tools',
    'memory_search',
    'memory_remember',
    'get_plan',
    'update_plan',
    'get_task_list',
    'update_task_list',
    'spawn_agent',
  ]
  assert.deepEqual(filterToolsForExecutionMode(names, 'read-only'), [
    'read',
    'grep',
    'discover_tools',
    'memory_search',
    'get_plan',
    'get_task_list',
  ])
  assert.deepEqual(filterToolsForExecutionMode(names, 'full-access'), names)
})

test('React exposes only read-only and full-access execution modes', async () => {
  const [session, controls, schedules] = await Promise.all([
    readFile(new URL('../../src/features/chat/FocusSession.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/chat/FocusRuntimeControls.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/schedules/SchedulesPage.tsx', import.meta.url), 'utf8'),
  ])
  assert.match(session, /<ExecutionModeSelect[\s\S]*?disabled=\{switchingPermission\}/)
  assert.doesNotMatch(session, /disabled=\{streaming \|\| switchingPermission\}/)
  assert.doesNotMatch(controls, /'workspace'/)
  assert.match(schedules, /\['full-access', 'read-only'\]/)
})

test('full-access execution does not request per-tool approval', () => {
  for (const [toolName, args] of [
    ['bash', { command: 'npm test' }],
    ['write', { path: '../outside.txt' }],
  ]) {
    assert.equal(
      permissionRequirement({
        mode: 'ignore',
        executionMode: 'full-access',
        cwd: process.cwd(),
        toolName,
        args,
      }),
      null,
    )
  }
})
