import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { resolve } from 'node:path'
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
  assert.equal(normalizeExecutionMode('workspace'), 'workspace')
  assert.equal(normalizeExecutionMode('full-access'), 'full-access')
  assert.equal(normalizeExecutionMode('unknown'), DEFAULT_EXECUTION_MODE)
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'ignore' }), 'full-access')
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'ask' }), 'workspace')
  assert.equal(migrateLegacyExecutionMode({ permissionMode: 'auto' }), 'workspace')
  assert.equal(
    migrateLegacyExecutionMode({ executionMode: 'read-only', permissionMode: 'ignore' }),
    'read-only',
  )
  assert.equal(permissionModeForExecutionMode('read-only'), 'ask')
  assert.equal(permissionModeForExecutionMode('workspace'), 'auto')
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
  assert.deepEqual(filterToolsForExecutionMode(names, 'workspace'), names)
})

test('React keeps execution mode switching available during active runs', async () => {
  const source = await readFile(
    new URL('../../src/features/chat/FocusSession.tsx', import.meta.url),
    'utf8',
  )
  assert.match(source, /<ExecutionModeSelect[\s\S]*?disabled=\{switchingPermission\}/)
  assert.doesNotMatch(source, /disabled=\{streaming \|\| switchingPermission\}/)
})

test('workspace file tools run directly while shell requires approval', () => {
  const cwd = process.cwd()
  const outside = resolve(cwd, '..', 'outside.txt')
  const shell = permissionRequirement({
    mode: 'ignore',
    executionMode: 'workspace',
    cwd,
    toolName: 'bash',
    args: { command: 'npm test' },
  })
  assert.equal(shell.risk, 'high')
  assert.match(shell.reason, /操作系统用户权限/)
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'workspace',
      cwd,
      toolName: 'read',
      args: { path: 'README.md' },
    }),
    null,
  )
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'workspace',
      cwd,
      toolName: 'write',
      args: { path: 'result.txt' },
    }),
    null,
  )
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'workspace',
      cwd,
      toolName: 'edit',
      args: { path: 'result.txt', edits: [] },
    }),
    null,
  )
  const outsideWrite = permissionRequirement({
    mode: 'ignore',
    executionMode: 'workspace',
    cwd,
    toolName: 'write',
    args: { path: outside },
  })
  assert.equal(outsideWrite.block, true)
  assert.match(outsideWrite.reason, /工作目录之外/)
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'full-access',
      cwd,
      toolName: 'write',
      args: { path: outside },
    }),
    null,
  )
})
