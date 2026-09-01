import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  DEFAULT_EXECUTION_MODE,
  filterToolsForExecutionMode,
  normalizeExecutionMode,
  permissionModeForExecutionMode,
} from '../security/execution-mode.mjs'
import { permissionRequirement } from '../services/session-permission-service.mjs'

test('execution modes normalize and default invalid values', () => {
  assert.equal(normalizeExecutionMode('approval-required'), 'approval-required')
  assert.equal(normalizeExecutionMode('workspace-write'), 'workspace-write')
  assert.equal(normalizeExecutionMode('full-access'), 'full-access')
  assert.equal(normalizeExecutionMode('workspace'), 'workspace-write')
  assert.equal(normalizeExecutionMode('unknown'), 'approval-required')
  assert.equal(normalizeExecutionMode('read-only'), 'approval-required')
  assert.equal(normalizeExecutionMode('unknown', 'workspace-write'), 'workspace-write')
  assert.equal(DEFAULT_EXECUTION_MODE, 'approval-required')
  assert.equal(permissionModeForExecutionMode('approval-required'), 'ask')
  assert.equal(permissionModeForExecutionMode('workspace-write'), 'auto')
  assert.equal(permissionModeForExecutionMode('full-access'), 'ignore')
})

test('approval-required execution exposes low-risk and approval-gated tools', () => {
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
    'mobile_device',
    'generate_visual',
    'get_plan',
    'update_plan',
    'get_task_list',
    'update_task_list',
    'spawn_agent',
    'followup_task',
    'interrupt_agent',
  ]
  // 审批模式：高危工具与内部中风险工具可见，但调用时仍需逐次审批。
  assert.deepEqual(filterToolsForExecutionMode(names, 'approval-required'), [
    'read',
    'grep',
    'edit',
    'bash',
    'discover_tools',
    'memory_search',
    'skill_create',
    'plugin_create',
    'mobile_device',
    'generate_visual',
    'get_plan',
    'update_plan',
    'get_task_list',
    'spawn_agent',
    'followup_task',
    'interrupt_agent',
  ])
  assert.deepEqual(filterToolsForExecutionMode(['get_goal', 'update_goal'], 'approval-required'), [
    'get_goal',
    'update_goal',
  ])
  assert.equal(
    filterToolsForExecutionMode(names, 'workspace-write').includes('plugin_create'),
    true,
  )
  assert.deepEqual(filterToolsForExecutionMode(names, 'full-access'), names)
})

test('React exposes approval-required, workspace-write, and full-access execution modes', async () => {
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
  assert.match(schedules, /type ScheduleExecutionMode = 'full-access'/)
  assert.doesNotMatch(schedules, /read-only/)
})

test('approval-visible multi-agent mutations still request per-call approval', () => {
  for (const toolName of [
    'spawn_agent',
    'followup_task',
    'interrupt_agent',
    'update_team_task',
    'run_team_workflow',
  ]) {
    assert.deepEqual(
      permissionRequirement({
        mode: 'ask',
        executionMode: 'approval-required',
        cwd: process.cwd(),
        toolName,
        args: {},
      }),
      {
        risk: 'medium',
        reason: `${toolName} 属于medium工具，需要确认后执行。`,
      },
    )
  }
})

test('Team roster and member messages stay low-risk in approval mode', () => {
  for (const toolName of ['send_team_message', 'list_team_members']) {
    assert.equal(
      permissionRequirement({
        mode: 'ask',
        executionMode: 'approval-required',
        cwd: process.cwd(),
        toolName,
        args: {},
      }),
      null,
    )
  }
})

test('plugin_create still requests approval outside full-access', () => {
  const requirement = permissionRequirement({
    mode: 'auto',
    executionMode: 'workspace-write',
    cwd: process.cwd(),
    toolName: 'plugin_create',
    args: { id: 'example.plugin', tools: [], entryCode: '' },
  })
  assert.deepEqual(requirement, {
    risk: 'high',
    reason: 'plugin_create 将创建并安装全局插件，需要确认后执行。',
  })
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
