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

test('workspace escalation and filesystem boundaries cannot be bypassed by legacy ignore mode', () => {
  const cwd = process.cwd()
  const outside = resolve(cwd, '..', 'outside.txt')
  const escalation = permissionRequirement({
    mode: 'ignore',
    executionMode: 'workspace',
    cwd,
    toolName: 'bash',
    args: { command: 'curl https://example.com', sandbox_permissions: 'require_escalated', justification: '需要访问受限网络' },
  })
  assert.equal(escalation.risk, '高风险')
  assert.equal(escalation.reason, '需要访问受限网络')
  assert.match(permissionRequirement({
    mode: 'ignore',
    executionMode: 'workspace',
    cwd,
    toolName: 'write',
    args: { path: outside },
  }).reason, /工作目录之外/)
  assert.equal(permissionRequirement({
    mode: 'ignore',
    executionMode: 'full-access',
    cwd,
    toolName: 'write',
    args: { path: outside },
  }), null)
})
