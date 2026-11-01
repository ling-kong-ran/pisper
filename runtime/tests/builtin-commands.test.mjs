// 内置斜杠命令：分组 + 子项的生成与菜单匹配（与 TUI 的 /model /effort /approval /run 对齐）。
import assert from 'node:assert/strict'
import test from 'node:test'
import {
  matchBuiltinCommands,
  runtimeSettingCommands,
} from '../../src/features/chat/builtin-commands.ts'

function commands(calls = []) {
  return runtimeSettingCommands({
    t: (key) => key,
    models: [
      { key: 'openai/gpt-5', label: 'GPT-5', providerName: 'OpenAI' },
      { key: 'anthropic/claude', label: 'Claude', providerName: 'Anthropic' },
    ],
    model: 'openai/gpt-5',
    onModel: (key) => calls.push(['model', key]),
    thinkingLevels: ['low', 'high'],
    thinkingLevel: 'high',
    thinkingLabel: (level) => `effort:${level}`,
    onThinking: (level) => calls.push(['effort', level]),
    executionModes: ['approval-required', 'workspace-write', 'full-access'],
    executionMode: 'approval-required',
    executionLabel: (mode) => `mode:${mode}`,
    onExecutionMode: (mode) => calls.push(['approval', mode]),
    runModes: ['plan', 'goal', 'team'],
    runMode: 'plan',
    runModeLabel: (mode) => `run:${mode}`,
    onRunMode: (mode) => calls.push(['run', mode]),
  })
}

test('a bare slash lists only top-level built-in commands, not every model', () => {
  assert.deepEqual(
    matchBuiltinCommands(commands(), '').map((command) => command.invocation),
    ['/model', '/effort', '/approval', '/run'],
  )
  assert.deepEqual(
    matchBuiltinCommands(commands(), 'ru').map((command) => command.invocation),
    ['/run'],
  )
  // 顶层命令是分组：没有 run，选中后由菜单补全参数。
  assert.equal(matchBuiltinCommands(commands(), 'run')[0].run, undefined)
})

test('a verb followed by a space lists that group and filters by the rest', () => {
  assert.deepEqual(
    matchBuiltinCommands(commands(), 'run ').map((command) => command.invocation),
    ['/run plan', '/run goal', '/run team'],
  )
  assert.deepEqual(
    matchBuiltinCommands(commands(), 'model anth').map((command) => command.invocation),
    ['/model anthropic/claude'],
  )
  // 描述同样参与过滤：输入显示名也能找到模型。
  assert.deepEqual(
    matchBuiltinCommands(commands(), 'model gpt-5').map((command) => command.description),
    ['GPT-5 · OpenAI'],
  )
  assert.deepEqual(matchBuiltinCommands(commands(), 'review src/app.ts'), [])
})

test('leaf commands run the page handler and mark the current selection', () => {
  const calls = []
  const list = commands(calls)
  const team = list.find((command) => command.invocation === '/run team')
  team.run()
  list.find((command) => command.invocation === '/approval full-access').run()
  assert.deepEqual(calls, [
    ['run', 'team'],
    ['approval', 'full-access'],
  ])
  assert.equal(list.find((command) => command.invocation === '/model openai/gpt-5').current, true)
  assert.equal(list.find((command) => command.invocation === '/effort high').current, true)
  assert.equal(list.find((command) => command.invocation === '/run team').current, false)
})

test('groups without options are omitted, and a single run mode hides /run', () => {
  const list = runtimeSettingCommands({
    t: (key) => key,
    models: [],
    model: '',
    onModel: () => {},
    thinkingLevels: [],
    thinkingLevel: '',
    thinkingLabel: (level) => level,
    onThinking: () => {},
    executionModes: ['approval-required'],
    executionMode: 'approval-required',
    executionLabel: (mode) => mode,
    onExecutionMode: () => {},
    runModes: ['plan'],
    runMode: 'plan',
    runModeLabel: (mode) => mode,
    onRunMode: () => {},
  })
  assert.deepEqual(
    list.map((command) => command.invocation),
    ['/approval', '/approval approval-required'],
  )
})
