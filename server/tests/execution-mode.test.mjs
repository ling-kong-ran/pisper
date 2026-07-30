import assert from 'node:assert/strict'
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
  assert.equal(migrateLegacyExecutionMode({ executionMode: 'read-only', permissionMode: 'ignore' }), 'read-only')
  assert.equal(permissionModeForExecutionMode('read-only'), 'ask')
  assert.equal(permissionModeForExecutionMode('workspace'), 'auto')
  assert.equal(permissionModeForExecutionMode('full-access'), 'ignore')
})

test('read-only execution exposes only low-risk analysis tools', () => {
  const names = ['read', 'grep', 'edit', 'bash', 'discover_tools', 'memory_search', 'memory_remember', 'get_task_list', 'spawn_agent']
  assert.deepEqual(filterToolsForExecutionMode(names, 'read-only'), [
    'read',
    'grep',
    'discover_tools',
    'memory_search',
    'get_task_list',
  ])
  assert.deepEqual(filterToolsForExecutionMode(names, 'workspace'), names)
})

test('workspace writes and shell require approval while reads stay direct', () => {
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
  const insideWrite = permissionRequirement({
    mode: 'ignore',
    executionMode: 'workspace',
    cwd,
    toolName: 'write',
    args: { path: 'result.txt' },
  })
  assert.equal(insideWrite.risk, 'high')
  assert.match(insideWrite.reason, /修改当前工作区/)
  const outsideWrite = permissionRequirement({
    mode: 'ignore',
    executionMode: 'workspace',
    cwd,
    toolName: 'write',
    args: { path: outside },
  })
  assert.equal(outsideWrite.block, true)
  assert.match(outsideWrite.reason, /工作目录之外/)
  assert.equal(permissionRequirement({
    mode: 'ignore',
    executionMode: 'full-access',
    cwd,
    toolName: 'write',
    args: { path: outside },
  }), null)
})
