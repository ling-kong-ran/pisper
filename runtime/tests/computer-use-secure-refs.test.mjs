import assert from 'node:assert/strict'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import {
  extractSecureOutlineTexts,
  scanOutlineForSecureRefs,
  SecureRefRegistry,
} from '../services/computer-use-secure-refs.mjs'
import { SessionPermissionService } from '../services/session-permission-service.mjs'

// 官方 outline 渲染样例：`@eN role/subrole "label" {actions} [annotations]`，
// 密码框的 role/subrole 必含 AXSecureTextField（macOS AX 原生标记）。
const LOGIN_OUTLINE = [
  '@e1 AXWindow "Login" { }',
  '  @e7 AXTextField/AXSecureTextField "" {type setText press}',
  '  @e8 AXTextField "username" {type setText}',
  '  @e9 AXButton "Sign In" {press}',
].join('\n')

function observeResult(text) {
  return { content: [{ type: 'text', text }] }
}

function makeService({ executionMode = '' } = {}) {
  return new SessionPermissionService({
    getMode: () => 'auto',
    getExecutionMode: () => executionMode,
  })
}

test('outline 扫描提取密码框 ref，普通字段不误报', () => {
  const refs = scanOutlineForSecureRefs(LOGIN_OUTLINE)
  assert.deepEqual([...refs], ['@e7'])
})

test('无标记文本与空输入安全返回', () => {
  assert.equal(scanOutlineForSecureRefs('@e1 AXButton "OK"').size, 0)
  assert.equal(scanOutlineForSecureRefs('').size, 0)
  assert.equal(scanOutlineForSecureRefs(null).size, 0)
})

test('结果文本递归提取覆盖 content/details 任意嵌套', () => {
  const result = {
    content: [{ type: 'text', text: 'state ok' }],
    details: { items: [{ text: LOGIN_OUTLINE }], image: 'AAAA'.repeat(10) },
  }
  const texts = extractSecureOutlineTexts(result)
  assert.equal(texts.length, 1)
  assert.match(texts[0], /AXSecureTextField/)
})

test('注册表 observe 登记 ref，非 outline 工具忽略', () => {
  const registry = new SecureRefRegistry()
  assert.equal(registry.observe('s1', 'observe_ui', observeResult(LOGIN_OUTLINE)), 1)
  assert.equal(registry.isSecureRef('s1', '@e7'), true)
  assert.equal(registry.isSecureRef('s1', '@e8'), false)
  // 其他会话隔离
  assert.equal(registry.isSecureRef('s2', '@e7'), false)
  // bash 结果即使碰巧含标记也不登记（数据源限定官方 outline 工具）
  assert.equal(registry.observe('s3', 'bash', observeResult(LOGIN_OUTLINE)), 0)
})

test('evaluateAct：显式 setText/typeText 命中密码框触发，普通字段放行', () => {
  const registry = new SecureRefRegistry()
  registry.observe('s1', 'act_ui', observeResult(LOGIN_OUTLINE))
  const hit = registry.evaluateAct('s1', 'act_ui', {
    actions: [{ action: 'setText', ref: '@e7', text: 'secret' }],
  })
  assert.equal(hit.via, 'ref')
  assert.match(hit.reason, /密码框/)
  assert.equal(
    registry.evaluateAct('s1', 'act_ui', {
      actions: [{ action: 'setText', ref: '@e8', text: 'user' }],
    }),
    null,
  )
  // 非 act_ui 工具不判定
  assert.equal(registry.evaluateAct('s1', 'click', { actions: [] }), null)
})

test('evaluateAct：焦点跟随——click 密码框后无 ref 输入/按键也触发', () => {
  const registry = new SecureRefRegistry()
  registry.observe('s1', 'observe_ui', observeResult(LOGIN_OUTLINE))
  // click 本身不写入，不触发，但更新焦点推断
  assert.equal(
    registry.evaluateAct('s1', 'act_ui', { actions: [{ action: 'click', ref: '@e7' }] }),
    null,
  )
  const typed = registry.evaluateAct('s1', 'act_ui', {
    actions: [{ action: 'typeText', text: 'hunter2' }],
  })
  assert.equal(typed.via, 'focus')
  // 焦点在密码框时回车=提交密码，同样触发
  const pressed = registry.evaluateAct('s1', 'act_ui', {
    actions: [{ action: 'keypress', keys: ['Return'] }],
  })
  assert.equal(pressed.via, 'focus')
  // 点击普通字段后焦点复位，无 ref 输入不再触发
  registry.evaluateAct('s1', 'act_ui', { actions: [{ action: 'click', ref: '@e8' }] })
  assert.equal(
    registry.evaluateAct('s1', 'act_ui', { actions: [{ action: 'typeText', text: 'abc' }] }),
    null,
  )
})

test('maskActArgs 对写入内容打码，结构保持', () => {
  const registry = new SecureRefRegistry()
  const masked = registry.maskActArgs({
    stateId: 'st-1',
    actions: [
      { action: 'setText', ref: '@e7', text: 'P@ssw0rd!' },
      { action: 'keypress', keys: ['Return'] },
      { action: 'click', ref: '@e9' },
    ],
  })
  assert.equal(masked.stateId, 'st-1')
  assert.equal(masked.actions[0].text, '••••••')
  assert.deepEqual(masked.actions[1].keys, ['•••'])
  assert.equal(masked.actions[2].ref, '@e9')
  assert.ok(!JSON.stringify(masked).includes('P@ssw0rd'))
})

test('auto 模式密码框 act 强制确认，审批载荷脱敏且不进审批记忆', async () => {
  const dir = await mkdtemp(join(tmpdir(), '/cu-secure-'))
  const service = makeService()
  service.observeComputerUseResult('s1', 'observe_ui', observeResult(LOGIN_OUTLINE))

  const captured = []
  service.requestApproval = async (request) => {
    captured.push(request)
    return { approved: true, reason: '' }
  }

  const args = {
    stateId: 'st-1',
    actions: [{ action: 'setText', ref: '@e7', text: 'P@ssw0rd!' }],
  }
  const first = await service.authorize({
    sessionId: 's1',
    cwd: dir,
    toolName: 'act_ui',
    toolCallId: 't1',
    args,
  })
  assert.equal(first, undefined, '批准后放行')
  assert.equal(captured.length, 1, 'auto 模式下密码框 act 必须弹审批')
  assert.equal(captured[0].risk, 'high')
  assert.match(captured[0].reason, /密码框/)
  // 审批事件里的 args 已脱敏：明文密码绝不出现在 SSE/落盘载荷中
  const wire = JSON.stringify(captured[0].args)
  assert.ok(!wire.includes('P@ssw0rd'), `审批载荷泄漏明文: ${wire}`)
  assert.match(wire, /•••/)

  // skipRemember：同一调用（相同 args 哈希）第二次仍必须重新确认
  const second = await service.authorize({
    sessionId: 's1',
    cwd: dir,
    toolName: 'act_ui',
    toolCallId: 't2',
    args,
  })
  assert.equal(second, undefined)
  assert.equal(captured.length, 2, '密码框审批不得进入 5 分钟记忆缓存')
})

test('auto 模式普通字段 act 不受关卡影响（免审批直通）', async () => {
  const service = makeService()
  service.observeComputerUseResult('s1', 'observe_ui', observeResult(LOGIN_OUTLINE))
  service.requestApproval = async () => {
    assert.fail('普通字段 act 不应触发审批')
  }
  const result = await service.authorize({
    sessionId: 's1',
    cwd: '/tmp',
    toolName: 'act_ui',
    toolCallId: 't1',
    args: { actions: [{ action: 'setText', ref: '@e8', text: 'user' }] },
  })
  assert.equal(result, undefined)
})

test('full-access 模式密码框 act 豁免，与既有完全信任语义一致', async () => {
  const service = makeService({ executionMode: 'full-access' })
  service.observeComputerUseResult('s1', 'observe_ui', observeResult(LOGIN_OUTLINE))
  service.requestApproval = async () => {
    assert.fail('full-access 不应触发审批')
  }
  const result = await service.authorize({
    sessionId: 's1',
    cwd: '/tmp',
    toolName: 'act_ui',
    toolCallId: 't1',
    args: { actions: [{ action: 'setText', ref: '@e7', text: 'secret' }] },
  })
  assert.equal(result, undefined)
})

test('拒绝密码框 act 时返回阻断与原因', async () => {
  const service = makeService()
  service.observeComputerUseResult('s1', 'observe_ui', observeResult(LOGIN_OUTLINE))
  service.requestApproval = async () => ({ approved: false, reason: '用户拒绝' })
  const result = await service.authorize({
    sessionId: 's1',
    cwd: '/tmp',
    toolName: 'act_ui',
    toolCallId: 't1',
    args: { actions: [{ action: 'typeText', ref: '@e7', text: 'x' }] },
  })
  assert.deepEqual(result, { block: true, reason: '用户拒绝' })
})
