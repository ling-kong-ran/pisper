import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import test from 'node:test'
import {
  DEFAULT_EXECUTION_MODE,
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

test('execution modes decide approvals, not which tools exist', async () => {
  // 第一层“按模式隐藏工具”已去掉：模块只剩模式归一化和默认权限映射。
  const module = await import('../security/execution-mode.mjs')
  assert.equal('filterToolsForExecutionMode' in module, false)
  const ask = (toolName, extra = {}) =>
    permissionRequirement({
      mode: 'ask',
      executionMode: 'approval-required',
      cwd: process.cwd(),
      toolName,
      args: {},
      ...extra,
    })
  // 需审批：中高风险工具照样可用，只是每次确认；联网搜索是只读公开搜索，按低风险直接放行。
  assert.equal(ask('web_search'), null)
  assert.equal(ask('memory_search'), null)
  assert.equal(ask('memory_remember').risk, 'medium')
  assert.equal(ask('browser_automation').risk, 'medium')
  assert.equal(ask('plugin_tool', { toolRisk: 'high', thirdPartyTool: true }).risk, 'high')
  // 工作区写：第三方插件工具仍逐次确认，内置的中风险工具直接放行。
  const auto = (toolName, extra = {}) =>
    permissionRequirement({
      mode: 'auto',
      executionMode: 'workspace-write',
      cwd: process.cwd(),
      toolName,
      args: {},
      ...extra,
    })
  assert.equal(auto('web_search'), null)
  assert.equal(auto('memory_remember'), null)
  assert.match(
    auto('plugin_tool', { toolRisk: 'high', thirdPartyTool: true }).reason,
    /third-party plugin/,
  )
  // 完全访问：不再询问。
  assert.equal(
    permissionRequirement({
      mode: 'ignore',
      executionMode: 'full-access',
      cwd: process.cwd(),
      toolName: 'plugin_tool',
      args: {},
      toolRisk: 'high',
      thirdPartyTool: true,
    }),
    null,
  )
})

test('React exposes approval-required, workspace-write, and full-access execution modes', async () => {
  // 审批模式在输入框的盾牌芯片里（规格 v2 第 2.5 节，ComposerModeChip）。
  const [session, controls, commands, schedules] = await Promise.all([
    readFile(new URL('../../src/features/chat/FocusSession.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/chat/ComposerModeChip.tsx', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/chat/use-session-commands.ts', import.meta.url), 'utf8'),
    readFile(new URL('../../src/features/schedules/SchedulesPage.tsx', import.meta.url), 'utf8'),
  ])
  assert.match(session, /<ComposerModeChip[\s\S]*?approvalDisabled=\{switchingPermission\}/)
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
        reason: `${toolName} is a medium-risk tool. Needs confirmation.`,
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
    reason: 'plugin_create will create and install a global plugin. Needs confirmation.',
  })
})

test('component_create follows the session permission mode like an ordinary file write', () => {
  // ask 审批一次；workspace-write（auto）与 full-access 直接放行；组件目录在工作区之外也不阻断。
  const base = { cwd: process.cwd(), toolName: 'component_create', args: { id: 'ci-status' } }
  assert.deepEqual(
    permissionRequirement({ ...base, mode: 'ask', executionMode: 'approval-required' }),
    {
      risk: 'high',
      reason:
        'component_create will write component ci-status (data folder/custom-ui/ci-status). Needs confirmation.',
    },
  )
  assert.equal(
    permissionRequirement({ ...base, mode: 'auto', executionMode: 'workspace-write' }),
    null,
  )
  assert.equal(
    permissionRequirement({ ...base, mode: 'ignore', executionMode: 'full-access' }),
    null,
  )
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
