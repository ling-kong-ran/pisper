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
  assert.equal(normalizeExecutionMode('approval-required'), 'approval-required')
  assert.equal(normalizeExecutionMode('workspace-write'), 'workspace-write')
  assert.equal(normalizeExecutionMode('full-access'), 'full-access')
  assert.equal(normalizeExecutionMode('workspace'), 'workspace-write')
  assert.equal(normalizeExecutionMode('unknown'), 'approval-required')
  assert.equal(DEFAULT_EXECUTION_MODE, 'approval-required')
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'ignore' }), 'full-access')
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'ask' }), 'full-access')
  assert.equal(migrateLegacyExecutionMode({ executionMode: 'workspace' }), 'full-access')
  assert.equal(
    migrateLegacyExecutionMode({ executionMode: 'read-only', permissionMode: 'ignore' }),
    'read-only',
  )
  assert.equal(permissionModeForExecutionMode('read-only'), 'ask')
  assert.equal(permissionModeForExecutionMode('approval-required'), 'ask')
  assert.equal(permissionModeForExecutionMode('workspace-write'), 'auto')
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
    'skill_create',
    'plugin_create',
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
  assert.deepEqual(filterToolsForExecutionMode(names, 'approval-required'), [
    'read',
    'grep',
    'edit',
    'bash',
    'discover_tools',
    'memory_search',
    'skill_create',
    'get_plan',
    'update_plan',
    'get_task_list',
  ])
  assert.equal(
    filterToolsForExecutionMode(names, 'workspace-write').includes('plugin_create'),
    false,
  )
  assert.deepEqual(filterToolsForExecutionMode(names, 'full-access'), names)
})

test('React exposes read-only, approval-required, workspace-write, and full-access execution modes', async () => {
  const [session, controls, commands, schedules] = await Promise.all([
    readFile(new URL('../../src/features/chat/FocusSession.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/chat/FocusRuntimeControls.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/chat/use-session-commands.ts', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/schedules/SchedulesPage.tsx', import.meta.url), 'utf8'),
  ])
  assert.match(session, /<ExecutionModeSelect[\s\S]*?disabled=\{switchingPermission\}/)
  assert.doesNotMatch(session, /disabled=\{streaming \|\| switchingPermission\}/)
  assert.match(controls, /'approval-required'/)
  assert.match(controls, /focusSession.approvalRequired/)
  assert.match(controls, /BadgeCheck/)
  assert.match(controls, /focusSession.workspaceWrite/)
  assert.doesNotMatch(commands, /stopTheActiveRunBeforeChangingTheExecutionMode/)
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
